/**
 * The crank-runner (SRS 08 §6, #141): one long-running process that does
 * every permissionless duty on a schedule, so no deadline depends on a
 * person remembering a command.
 *
 * Each tick:
 *   1. phases   — every live auction-engine session (discovered on chain)
 *                 is driven forward: clear / allocate / close_rofr /
 *                 settle / finish / expire (#149);
 *   2. refunds  — every finished or failed session whose rails CashSession
 *                 still has open escrows gets its refunds (AE-P10);
 *   3. seasoning — credit-gate `merge_expired` for each configured mint;
 *   4. distribution — each configured tranche pool whose period is due is
 *                 paid from the runner wallet's USDC (TE-P4).
 *
 * Stateless like every crank: a restart re-reads the chain. The kill switch
 * (`CRANK_KILL_FILE`, the AC-11 drill) pauses steps 1, 3 and 4 while it
 * exists; refunds (step 2) keep running, because refunds are never pausable
 * (AE-P10). The file is checked once at the start of each tick, so a tick
 * already in progress finishes. The protocol's liveness never depends on
 * this runner: every step is permissionless.
 */
import { existsSync, readFileSync } from "node:fs";
import { address, getBase64Encoder, type Address, type Base64EncodedBytes } from "@solana/kit";
import { refundAll, type Outcome } from "./crank.ts";
import { distribute, distributionDue, loadPool } from "./distribute.ts";
import { clockNow, fetchDecoded, ownerTokenAccount, tokenBalance } from "./chain.ts";
import type { Struct } from "./idl.ts";
import { configPda } from "./pda.ts";
import { auction, AUCTION, runPhases, type AuctionSession } from "./phases.ts";
import { seasonAll } from "./seasoning.ts";
import type { Ctx } from "./send.ts";

export interface RunnerConfig {
  intervalSeconds: number;
  /** Discover live auction-engine sessions on chain (default true). */
  auctions?: boolean;
  seasoningMints?: string[];
  distribution?: { pool: string; cashPerPeriod: string; minIntervalSeconds: number }[];
}

export function parseRunnerConfig(json: string): RunnerConfig {
  const c = JSON.parse(json) as RunnerConfig;
  if (!(c.intervalSeconds > 0)) throw new Error("intervalSeconds must be > 0");
  for (const m of c.seasoningMints ?? []) address(m);
  for (const d of c.distribution ?? []) {
    address(d.pool);
    if (BigInt(d.cashPerPeriod) <= 0n) throw new Error(`cashPerPeriod must be > 0 for ${d.pool}`);
    if (!(d.minIntervalSeconds >= 0)) throw new Error(`minIntervalSeconds must be ≥ 0 for ${d.pool}`);
  }
  return c;
}

export function killed(): boolean {
  const f = process.env.CRANK_KILL_FILE;
  return !!f && existsSync(f);
}

/** Every auction-engine session account, decoded. */
export async function listAuctionSessions(ctx: Ctx): Promise<{ address: Address; session: AuctionSession }[]> {
  const disc = auction.idl.accounts.find((a) => a.name === "Session")!.discriminator;
  const b64 = btoa(String.fromCharCode(...disc)) as Base64EncodedBytes;
  const rows = await ctx.rpc
    .getProgramAccounts(AUCTION, {
      encoding: "base64",
      commitment: "confirmed",
      filters: [{ memcmp: { offset: 0n, bytes: b64, encoding: "base64" } }],
    })
    .send();
  const enc = getBase64Encoder();
  return rows.flatMap((r) => {
    const s = auction.decodeAccount<AuctionSession>("Session", new Uint8Array(enc.encode(r.account.data[0])));
    return s ? [{ address: r.pubkey, session: s }] : [];
  });
}

export async function tick(ctx: Ctx, cfg: RunnerConfig): Promise<Outcome[]> {
  const kill = killed();
  const out: Outcome[] = kill ? [{ target: "runner", result: "skipped", detail: "kill switch engaged: refunds only" }] : [];
  if (cfg.auctions !== false) {
    for (const { address: a, session: s } of await listAuctionSessions(ctx)) {
      if (s.phase === "Printed" || s.phase === "Failed") {
        const cs = await fetchDecoded<Struct & { open_escrows: number }>(ctx.rpc, "CashSession", s.rails_session);
        if (cs && cs.open_escrows > 0) out.push(...(await refundAll(ctx, s.rails_session)));
        continue;
      }
      if (!kill) out.push(...(await runPhases(ctx, a, ctx.payer, { maxSteps: 16 })).filter((o) => o.result !== "already-done"));
    }
  }
  if (kill) return out;
  for (const m of cfg.seasoningMints ?? []) {
    out.push(...(await seasonAll(ctx, address(m))).filter((o) => o.result !== "already-done" && o.result !== "not-yet"));
  }
  if ((cfg.distribution ?? []).length > 0) {
    const now = await clockNow(ctx.rpc);
    const railsCfg = await fetchDecoded<Struct & { usdc_mint: Address }>(ctx.rpc, "RailsConfig", await configPda());
    for (const d of cfg.distribution ?? []) {
      const pool = address(d.pool);
      const cash = BigInt(d.cashPerPeriod);
      const p = await loadPool(ctx, pool);
      if (!distributionDue(p, now, BigInt(d.minIntervalSeconds))) continue;
      const usdc = railsCfg ? await ownerTokenAccount(ctx.rpc, ctx.payer.address, railsCfg.usdc_mint) : null;
      const bal = usdc ? await tokenBalance(ctx.rpc, usdc) : null;
      if (bal === null || bal < cash) {
        out.push({ target: `pool ${pool}`, result: "skipped", detail: `runner wallet holds ${bal ?? 0n} < ${cash} for the period` });
        continue;
      }
      out.push(await distribute(ctx, pool, cash, ctx.payer));
    }
  }
  return out;
}

/** Run forever (or once): log each tick's outcomes. */
export async function run(ctx: Ctx, cfg: RunnerConfig, opts: { once?: boolean } = {}): Promise<Outcome[]> {
  for (;;) {
    let outcomes: Outcome[];
    try {
      outcomes = await tick(ctx, cfg);
    } catch (e) {
      outcomes = [{ target: "runner", result: "failed", detail: (e as Error).message }];
    }
    for (const o of outcomes) ctx.log(`  ${o.result.padEnd(12)} ${o.target}${o.detail ? ` — ${o.detail}` : ""}`);
    if (opts.once) return outcomes;
    await new Promise((r) => setTimeout(r, cfg.intervalSeconds * 1000));
  }
}

export function loadRunnerConfig(path: string): RunnerConfig {
  return parseRunnerConfig(readFileSync(path, "utf8"));
}
