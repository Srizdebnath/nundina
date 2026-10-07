/**
 * nundina off-chain services (SRS 08 §6).
 *
 *   npm run svc -- indexer [--once] [--interval 15] [--programs credit-gate,payment-rails]
 *                                                       # RPC backfill / gap-fill loop
 *   npm run svc -- webhook --port 8787                  # Helius raw-tx webhook receiver (WEBHOOK_SECRET)
 *   npm run svc -- api --port 8788                      # read-only JSON API
 *   npm run svc -- nav --mint <mint> --source sleeve:<usdc acct>|treasury:<pda>
 *                      [--fee-bps 10] [--out evidence/nav] [--dry-run]     # PUBLISHER keypair signs
 *   npm run svc -- treasury status --source treasury:<pda> | --source sleeve:<usdc acct> --mint <mint>
 *   npm run svc -- treasury reconcile --source treasury:<pda>             # PAYER keypair pays
 *
 * Env: RPC_URL (default http://127.0.0.1:8899), DATABASE_URL (postgres://…
 * or pglite:<dir> | pglite:memory; default pglite:./.data/indexer),
 * WEBHOOK_SECRET, PUBLISHER / PAYER (keypair paths, default
 * ~/.config/solana/id.json). The crank-runner and the session evidence
 * exporter live with the crank: `payment-rails/crank` (`run`, `export --session`).
 */
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { homedir } from "node:os";
import { parseArgs } from "node:util";
import { address, createKeyPairSignerFromBytes, type KeyPairSigner } from "@solana/kit";
import { apiServer } from "./api.ts";
import { openDb } from "./db.ts";
import { backfillAll, webhookHandler } from "./indexer.ts";
import { publish } from "./nav-publisher.ts";
import { makeRpc, RPC_URL } from "./rpc.ts";
import { formatTreasury, reconcileOnChain, treasuryStatus } from "./treasury.ts";

const { positionals, values } = parseArgs({
  allowPositionals: true,
  options: {
    once: { type: "boolean" },
    interval: { type: "string" },
    port: { type: "string" },
    mint: { type: "string" },
    source: { type: "string" },
    "fee-bps": { type: "string" },
    out: { type: "string" },
    "dry-run": { type: "boolean" },
    programs: { type: "string" },
  },
});

const need = (k: keyof typeof values): string => {
  const v = values[k];
  if (typeof v !== "string") throw new Error(`--${k} is required`);
  return v;
};

async function keypair(envVar: string): Promise<KeyPairSigner> {
  const p = (process.env[envVar] ?? "~/.config/solana/id.json").replace(/^~(?=\/)/, homedir());
  return createKeyPairSignerFromBytes(Uint8Array.from(JSON.parse(readFileSync(p, "utf8"))));
}

const dbUrl = () => process.env.DATABASE_URL ?? "pglite:./.data/indexer";

async function main(): Promise<boolean> {
  const rpc = makeRpc(RPC_URL);
  switch (positionals[0]) {
    case "indexer": {
      const db = await openDb(dbUrl());
      const every = Number(values.interval ?? 15) * 1000;
      for (;;) {
        const r = await backfillAll(rpc, db, values.programs?.split(","));
        const moved = Object.entries(r).filter(([, v]) => v.txs > 0);
        console.log(`${new Date().toISOString()} ${moved.length ? moved.map(([k, v]) => `${k}: ${v.txs} tx / ${v.events} ev`).join(" · ") : "up to date"}`);
        if (values.once) {
          await db.close();
          return true;
        }
        await new Promise((res) => setTimeout(res, every));
      }
    }
    case "webhook": {
      const secret = process.env.WEBHOOK_SECRET ?? "";
      if (!secret) throw new Error("WEBHOOK_SECRET is required");
      const db = await openDb(dbUrl());
      const h = webhookHandler(db, secret);
      createServer((req, res) => void h(req, res)).listen(Number(values.port ?? 8787));
      console.log(`webhook receiver on :${values.port ?? 8787}`);
      return new Promise(() => {});
    }
    case "api": {
      const db = await openDb(dbUrl());
      apiServer(db).listen(Number(values.port ?? 8788));
      console.log(`api on :${values.port ?? 8788}`);
      return new Promise(() => {});
    }
    case "nav": {
      const r = await publish(rpc, {
        mint: address(need("mint")),
        source: need("source"),
        feeBps: BigInt(values["fee-bps"] ?? 10),
        publisher: values["dry-run"] ? null : await keypair("PUBLISHER"),
        out: values.out,
      });
      const c = r.computed;
      console.log(
        [
          `feed            ${c.feed}`,
          `supply          ${c.inputs.supply_base} · treasury ${c.inputs.treasury_micros} µUSDC (${c.inputs.treasury_source} ${c.inputs.treasury_account}) @ slot ${c.inputs.slot}`,
          `Δt              ${c.inputs.delta_seconds} s · fee ${c.inputs.fee_bps} bps/yr`,
          `NAV gross       ${c.gross} pico`,
          `fee             ${c.fee} pico`,
          `NAV published   ${c.published} pico (${Number(c.published) / 1e12} USDC/unit)`,
          `inputs_hash     ${Buffer.from(c.hash).toString("hex")}`,
          r.signature ? `publish_nav     ${r.signature}` : "dry run — nothing sent",
          r.evidence ? `evidence        ${r.evidence}` : "",
        ]
          .filter(Boolean)
          .join("\n"),
      );
      return true;
    }
    case "treasury": {
      const source = need("source");
      if (positionals[1] === "status") {
        const r = await treasuryStatus(rpc, { source, mint: values.mint ? address(values.mint) : undefined });
        console.log(formatTreasury(r));
        return r.backed;
      }
      if (positionals[1] === "reconcile") {
        if (!source.startsWith("treasury:")) throw new Error("reconcile needs --source treasury:<pda> (the on-chain check is the program's)");
        const r = await reconcileOnChain(rpc, await keypair("PAYER"), address(source.slice("treasury:".length)));
        console.log(`reconcile ${r.signature}: ${r.backed ? "BACKED" : "NOT BACKED"}`);
        return r.backed;
      }
      throw new Error("usage: svc treasury <status|reconcile> --source …");
    }
    default:
      console.error("usage: svc <indexer|webhook|api|nav|treasury> — see src/cli.ts");
      return false;
  }
}

main().then(
  (ok) => process.exit(ok ? 0 : 1),
  (e) => {
    console.error((e as Error).message);
    process.exit(1);
  },
);
