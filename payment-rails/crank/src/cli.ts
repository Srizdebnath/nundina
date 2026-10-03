/**
 * payment-rails crank CLI.
 *
 *   npm run crank -- refunds   --session <addr>              # any wallet can run this
 *   npm run crank -- settle    --session <addr> --plan fills.json [--max-sends N]
 *   npm run crank -- finish    --session <addr>
 *   npm run crank -- reconcile --session <addr> | --facility <addr>
 *   npm run crank -- export    --out <dir> [--limit N]          # signed (#127)
 *   npm run crank -- verify-export --dir <dir> [--signer <addr>] # keyless, offline
 *
 * Env: RPC_URL (default localhost), WS_URL, KEYPAIR (default
 * ~/.config/solana/id.json), EVIDENCE_KEYPAIR (signs exports; default
 * KEYPAIR), PRIORITY_FEE_MIN/MAX (µlamports/CU),
 * MAX_ATTEMPTS, RETRY_BASE_MS. Exit code 1 on any failed action or a
 * reconciliation that isn't clean.
 */
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { address } from "@solana/kit";
import { finish, refundAll, settlePlan, type FillPlan, type Outcome } from "./crank.ts";
import { loadKeypair, makeCtx } from "./ctx.ts";
import { exportEvidence, verifyEvidence } from "./evidence.ts";
import { formatReport, reconcileFacility, reconcileSession } from "./ledger.ts";

const { positionals, values } = parseArgs({
  allowPositionals: true,
  options: {
    session: { type: "string" },
    facility: { type: "string" },
    plan: { type: "string" },
    "max-sends": { type: "string" },
    out: { type: "string" },
    limit: { type: "string" },
    dir: { type: "string" },
    signer: { type: "string" },
  },
});

function need(name: keyof typeof values): string {
  const v = values[name];
  if (typeof v !== "string") throw new Error(`--${name} is required`);
  return v;
}

function summarize(outcomes: Outcome[]): boolean {
  for (const o of outcomes) console.log(`  ${o.result.padEnd(12)} ${o.target}${o.detail ? ` — ${o.detail}` : ""}`);
  const failed = outcomes.filter((o) => o.result === "failed").length;
  console.log(`${outcomes.length} targets, ${failed} failed`);
  return failed === 0;
}

export function parsePlan(json: string): FillPlan[] {
  const raw = JSON.parse(json) as {
    index: number;
    buyer: string;
    seller: string;
    qty: string | number;
    price: string | number;
    sellerUsdc?: string;
    buyerAsset?: string;
  }[];
  return raw.map((f) => ({
    index: f.index,
    buyer: address(f.buyer),
    seller: address(f.seller),
    qty: BigInt(f.qty),
    price: BigInt(f.price),
    sellerUsdc: f.sellerUsdc ? address(f.sellerUsdc) : undefined,
    buyerAsset: f.buyerAsset ? address(f.buyerAsset) : undefined,
  }));
}

async function main(): Promise<boolean> {
  const cmd = positionals[0];
  if (cmd === "verify-export") {
    // Needs neither a key nor an RPC: an auditor runs it on the files alone.
    const r = await verifyEvidence(need("dir"), values.signer);
    console.log(r.ok ? `evidence OK — signed by ${r.signer}` : `evidence REJECTED:\n  ${r.problems.join("\n  ")}`);
    return r.ok;
  }
  const ctx = await makeCtx();
  switch (cmd) {
    case "refunds":
      return summarize(await refundAll(ctx, address(need("session"))));
    case "settle": {
      const plan = parsePlan(readFileSync(need("plan"), "utf8"));
      const maxSends = values["max-sends"] ? Number(values["max-sends"]) : undefined;
      return summarize(await settlePlan(ctx, address(need("session")), ctx.payer, plan, { maxSends }));
    }
    case "finish":
      return summarize([await finish(ctx, address(need("session")), ctx.payer)]);
    case "reconcile": {
      const r = values.facility
        ? await reconcileFacility(ctx.rpc, address(values.facility))
        : await reconcileSession(ctx.rpc, address(need("session")));
      console.log(formatReport(r));
      return r.ok;
    }
    case "export": {
      const signer = await loadKeypair(process.env.EVIDENCE_KEYPAIR ?? process.env.KEYPAIR ?? "~/.config/solana/id.json");
      const m = await exportEvidence(ctx.rpc, need("out"), signer, { limit: values.limit ? Number(values.limit) : undefined });
      console.log(JSON.stringify(m, null, 2));
      return true;
    }
    default:
      console.error("usage: crank <refunds|settle|finish|reconcile|export|verify-export> [options] — see src/cli.ts");
      return false;
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main()
    .then((ok) => process.exit(ok ? 0 : 1))
    .catch((e) => {
      console.error(e instanceof Error ? e.message : e);
      process.exit(1);
    });
}
