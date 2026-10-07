/**
 * Transaction sending with priority fees and bounded retries.
 *
 * - Priority fee: p75 of `getRecentPrioritizationFees` for the rails
 *   program, clamped to [min, max] micro-lamports per CU.
 * - Retries: only *transient* failures (expired blockhash, network errors,
 *   rate limits) are retried, with exponential backoff and a fresh
 *   blockhash each attempt. Each retry **escalates** the fee ×1.5 over the
 *   oracle's pick (at least `retryFloorMicroLamports`, clamped to max), and
 *   both the fee and the delay get random **jitter**, so a crank losing to
 *   a fee spike outbids it and retrying cranks don't move in lockstep
 *   (SRS 08 §6, doc 05 D11). A program error is final and returned typed —
 *   callers decide whether it means "already done" (idempotency).
 */
import {
  appendTransactionMessageInstructions,
  compressTransactionMessageUsingAddressLookupTables,
  createTransactionMessage,
  getSignatureFromTransaction,
  pipe,
  sendAndConfirmTransactionFactory,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Instruction,
  type Rpc,
  type RpcSubscriptions,
  type SolanaRpcApi,
  type SolanaRpcSubscriptionsApi,
  type TransactionSigner,
  type AddressesByLookupTableAddress,
} from "@solana/kit";
import {
  getSetComputeUnitLimitInstruction,
  getSetComputeUnitPriceInstruction,
} from "@solana-program/compute-budget";
import { errorName, PROGRAM_ID } from "./idl.ts";

export interface FeeConfig {
  minMicroLamports: bigint;
  maxMicroLamports: bigint;
  percentile: number; // 0..100
  /** Fee used on a retry when the oracle says 0, so escalation has a base. */
  retryFloorMicroLamports: bigint;
  /** ± jitter on fee and delay, in percent (0 disables). */
  jitterPct: number;
}

export interface Ctx {
  rpc: Rpc<SolanaRpcApi>;
  rpcSubscriptions: RpcSubscriptions<SolanaRpcSubscriptionsApi>;
  /** Fee payer (and default signer). */
  payer: TransactionSigner;
  fee: FeeConfig;
  maxAttempts: number;
  baseDelayMs: number;
  log: (msg: string) => void;
}

export type SendResult =
  | { status: "sent"; signature: string; attempts: number; priceMicroLamports: bigint }
  | {
      status: "failed";
      /** Custom program error code, when the program rejected it. */
      code?: number;
      name?: string;
      /** An `init` hit an existing account (idempotency signal). */
      alreadyInUse: boolean;
      transient: boolean;
      message: string;
      attempts: number;
    };

export const DEFAULT_FEE: FeeConfig = {
  minMicroLamports: 0n,
  maxMicroLamports: 50_000n,
  percentile: 75,
  retryFloorMicroLamports: 1_000n,
  jitterPct: 10,
};

/**
 * D11: the fee for `attempt` (1-based). Attempt 1 is the oracle pick; each
 * retry multiplies by 1.5 (from at least `retryFloorMicroLamports`), adds
 * ±`jitterPct`, and clamps to [min, max]. `rand` ∈ [0, 1). Pure, unit tested.
 */
export function escalatedFee(base: bigint, attempt: number, cfg: FeeConfig, rand: number): bigint {
  const clamp = (v: bigint) => (v < cfg.minMicroLamports ? cfg.minMicroLamports : v > cfg.maxMicroLamports ? cfg.maxMicroLamports : v);
  if (attempt <= 1) return clamp(base);
  let fee = base > cfg.retryFloorMicroLamports ? base : cfg.retryFloorMicroLamports;
  for (let i = 1; i < attempt && fee < cfg.maxMicroLamports; i++) fee = (fee * 3n) / 2n;
  const j = BigInt(Math.round((rand * 2 - 1) * cfg.jitterPct * 100)); // basis points of fee
  return clamp(fee + (fee * j) / 10_000n);
}

/** Exponential backoff with ±`jitterPct` jitter. Pure, unit tested. */
export function backoffDelayMs(baseMs: number, attempt: number, jitterPct: number, rand: number): number {
  const d = baseMs * 2 ** (attempt - 1);
  return Math.max(0, Math.round(d * (1 + ((rand * 2 - 1) * jitterPct) / 100)));
}

/** p-th percentile of recent fees, clamped. Pure — unit tested. */
export function pickPriorityFee(fees: bigint[], cfg: FeeConfig): bigint {
  if (fees.length === 0) return cfg.minMicroLamports;
  const sorted = [...fees].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  const idx = Math.min(sorted.length - 1, Math.floor((cfg.percentile / 100) * sorted.length));
  const p = sorted[idx]!;
  return p < cfg.minMicroLamports ? cfg.minMicroLamports : p > cfg.maxMicroLamports ? cfg.maxMicroLamports : p;
}

export async function priorityFee(ctx: Ctx): Promise<bigint> {
  try {
    const recent = await ctx.rpc.getRecentPrioritizationFees([PROGRAM_ID]).send();
    return pickPriorityFee(recent.map((r) => BigInt(r.prioritizationFee)), ctx.fee);
  } catch {
    return ctx.fee.minMicroLamports; // fee oracle down ≠ crank down
  }
}

// ── error classification (pure — unit tested) ───────────────────────────

const TRANSIENT =
  /blockhash not found|block height exceeded|expired|timed? ?out|fetch failed|ECONNRESET|ECONNREFUSED|socket|network|429|too many requests|node is behind|service unavailable/i;

/** Collect every string reachable from an error (message, logs, causes). */
function strings(e: unknown, depth = 0, seen = new Set<unknown>()): string[] {
  if (e === null || e === undefined || depth > 6 || seen.has(e)) return [];
  if (typeof e === "string") return [e];
  if (typeof e !== "object") return [];
  seen.add(e);
  const out: string[] = [];
  const o = e as Record<string, unknown>;
  if (typeof o.message === "string") out.push(o.message);
  for (const v of Object.values(o)) {
    if (Array.isArray(v)) for (const x of v) out.push(...strings(x, depth + 1, seen));
    else if (typeof v === "object") out.push(...strings(v, depth + 1, seen));
  }
  if ("cause" in o) out.push(...strings(o.cause, depth + 1, seen));
  return out;
}

export function classify(e: unknown): Omit<Extract<SendResult, { status: "failed" }>, "attempts"> {
  const all = strings(e);
  const text = all.join("\n");
  let code: number | undefined;
  const anchor = text.match(/Error Number: (\d+)/);
  const custom = text.match(/custom program error: 0x([0-9a-f]+)/i);
  if (anchor) code = Number(anchor[1]);
  else if (custom) code = parseInt(custom[1]!, 16);
  const alreadyInUse = /already in use/i.test(text);
  const programFailure = code !== undefined || alreadyInUse || /Program \w+ failed/.test(text);
  return {
    status: "failed",
    code,
    name: code !== undefined ? errorName(code) : undefined,
    alreadyInUse,
    transient: !programFailure && TRANSIENT.test(text),
    message: all[0] ?? String(e),
  };
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Send `instructions` with a compute-unit limit and a priority fee;
 * retry transient failures with backoff. Never throws for chain errors.
 */
export async function send(
  ctx: Ctx,
  label: string,
  instructions: Instruction[],
  opts: { cuLimit?: number; lookupTables?: AddressesByLookupTableAddress } = {},
): Promise<SendResult> {
  const sendAndConfirm = sendAndConfirmTransactionFactory({
    rpc: ctx.rpc,
    rpcSubscriptions: ctx.rpcSubscriptions,
  });
  let last: ReturnType<typeof classify> | undefined;
  let attempts = 0;
  for (let attempt = 1; attempt <= ctx.maxAttempts; attempt++) {
    attempts = attempt;
    const price = escalatedFee(await priorityFee(ctx), attempt, ctx.fee, Math.random());
    try {
      const { value: blockhash } = await ctx.rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
      const msg = pipe(
        createTransactionMessage({ version: 0 }),
        (m) => setTransactionMessageFeePayerSigner(ctx.payer, m),
        (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
        (m) =>
          appendTransactionMessageInstructions(
            [
              getSetComputeUnitLimitInstruction({ units: opts.cuLimit ?? 400_000 }),
              getSetComputeUnitPriceInstruction({ microLamports: price }),
              ...instructions,
            ],
            m,
          ),
        (m) => (opts.lookupTables ? compressTransactionMessageUsingAddressLookupTables(m, opts.lookupTables) : m),
      );
      const signed = await signTransactionMessageWithSigners(msg);
      await sendAndConfirm(signed as Parameters<typeof sendAndConfirm>[0], { commitment: "confirmed" });
      const signature = getSignatureFromTransaction(signed);
      ctx.log(`✓ ${label} ${signature.slice(0, 16)}… (attempt ${attempt}, fee ${price} µL/CU)`);
      return { status: "sent", signature, attempts: attempt, priceMicroLamports: price };
    } catch (e) {
      last = classify(e);
      if (!last.transient) break;
      const delay = backoffDelayMs(ctx.baseDelayMs, attempt, ctx.fee.jitterPct, Math.random());
      ctx.log(`↻ ${label}: transient (${last.message.slice(0, 80)}); retry in ${delay} ms`);
      await sleep(delay);
    }
  }
  const f = last!;
  ctx.log(`✗ ${label}: ${f.name ?? (f.alreadyInUse ? "already in use" : f.message.slice(0, 120))}`);
  return { ...f, attempts };
}
