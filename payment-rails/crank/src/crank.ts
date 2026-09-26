/**
 * The crank: settlement and refunds, both idempotent.
 *
 * Idempotency is anchored on-chain, not in crank memory, so a crank can be
 * killed and restarted (or replaced by any participant) at any point:
 * - a fill is done iff its receipt PDA exists → re-running a plan skips it;
 *   a send that races a landed duplicate fails with "already in use" and is
 *   counted as done;
 * - an escrow is refunded iff its account is gone → a refund that races
 *   another caller's refund is counted as done.
 * Retried sends are safe for the same reason: the program can't pay twice.
 */
import type { Address, TransactionSigner } from "@solana/kit";
import { clockNow, exists, fetchDecoded, listByKey, ownerTokenAccount, tokenProgramOf, type R } from "./chain.ts";
import { ix, type Struct } from "./idl.ts";
import { assetVaultPda, configPda, escrowPda, receiptPda, vaultPda } from "./pda.ts";
import { refundRoute, type SessionStatus } from "./rules.ts";
import { send, type Ctx } from "./send.ts";

export interface CashSession extends Struct {
  operator: Address;
  usdc_mint: Address;
  reserve: Address;
  fee_receiver: Address;
  asset_mint: Address;
  reveal_end: bigint;
  settle_deadline: bigint;
  failed_at: bigint;
  status: SessionStatus;
  open_escrows: number;
  total_cash: bigint;
  total_bonds: bigint;
  total_asset: bigint;
  fills_settled: number;
  volume_settled: bigint;
  gross_settled: bigint;
}

export interface Escrow extends Struct {
  session: Address;
  owner: Address;
  vault: Address;
  side: "Buy" | "Sell";
  cash: bigint;
  asset: bigint;
  asset_vault: Address;
  bond: bigint;
  revealed: boolean;
}

export async function loadSession(rpc: R, session: Address): Promise<CashSession> {
  const s = await fetchDecoded<CashSession>(rpc, "CashSession", session);
  if (!s) throw new Error(`session ${session} not found`);
  return s;
}

/** Escrow accounts of a session (Escrow.session is at offset 8). */
export const listEscrows = (rpc: R, session: Address) => listByKey<Escrow>(rpc, "Escrow", 8, session);

export interface Outcome {
  target: string;
  result: "sent" | "already-done" | "not-yet" | "skipped" | "failed";
  detail?: string;
  signature?: string;
}

// ── refunds ──────────────────────────────────────────────────────────────

/**
 * Refund every escrow of `session` that the program would refund now.
 * `ctx.payer` signs as `caller` — refunds are permissionless, so any
 * participant's wallet can run this (the kill-the-crank guarantee).
 */
export async function refundAll(ctx: Ctx, session: Address): Promise<Outcome[]> {
  const s = await loadSession(ctx.rpc, session);
  const now = await clockNow(ctx.rpc);
  const times = { status: s.status, revealEnd: s.reveal_end, settleDeadline: s.settle_deadline, failedAt: s.failed_at };
  const [usdcProgram, assetProgram] = await Promise.all([
    tokenProgramOf(ctx.rpc, s.usdc_mint),
    tokenProgramOf(ctx.rpc, s.asset_mint),
  ]);
  const out: Outcome[] = [];
  for (const { address: escrow, data: e } of await listEscrows(ctx.rpc, session)) {
    const target = `${e.side === "Buy" ? "bid" : "ask"} ${e.owner}`;
    const route = refundRoute(times, e.revealed, now);
    if (route === null) {
      out.push({ target, result: "not-yet", detail: e.revealed ? "awaiting settlement" : "reveals open" });
      continue;
    }
    const ownerUsdc = await ownerTokenAccount(ctx.rpc, e.owner, s.usdc_mint);
    if (!ownerUsdc) {
      out.push({ target, result: "skipped", detail: "owner has no USDC account to refund into" });
      continue;
    }
    const common = {
      caller: ctx.payer,
      session,
      escrow,
      vault: e.vault,
      owner: e.owner,
      reserve: s.reserve,
      usdc_mint: s.usdc_mint,
      token_program: usdcProgram,
    };
    let instruction;
    if (e.side === "Buy") {
      instruction = ix("refund", { ...common, owner_token: ownerUsdc });
    } else {
      const ownerAsset = await ownerTokenAccount(ctx.rpc, e.owner, s.asset_mint);
      if (!ownerAsset) {
        out.push({ target, result: "skipped", detail: "owner has no asset account to return asset into" });
        continue;
      }
      instruction = ix("refund_ask", {
        ...common,
        asset_vault: e.asset_vault,
        owner_usdc: ownerUsdc,
        owner_asset: ownerAsset,
        asset_mint: s.asset_mint,
        asset_token_program: assetProgram,
      });
    }
    const r = await send(ctx, `refund ${target.slice(0, 20)}… (bond → ${route === "ToReserve" ? "reserve" : "owner"})`, [instruction], { cuLimit: 120_000 });
    if (r.status === "sent") out.push({ target, result: "sent", signature: r.signature, detail: `bond → ${route}` });
    else if (!(await exists(ctx.rpc, escrow))) out.push({ target, result: "already-done", detail: "refunded concurrently" });
    else out.push({ target, result: "failed", detail: r.name ?? r.message });
  }
  return out;
}

// ── settlement ───────────────────────────────────────────────────────────

/** One fill of the auction's allocation (from the engine's clearing). */
export interface FillPlan {
  index: number;
  buyer: Address;
  seller: Address;
  qty: bigint;
  price: bigint;
  disclosureHash?: Uint8Array;
  /** Seller's USDC account to pay; looked up (lowest address) if absent. */
  sellerUsdc?: Address;
  /** Buyer's asset account to deliver to; looked up if absent. */
  buyerAsset?: Address;
}

/**
 * Settle `plan` in order, one fill per transaction. Fills whose receipt
 * already exists are skipped, so re-running after a crash resumes exactly
 * where the last run stopped. A failing fill is reported and the rest
 * continue (one bad fill must not block the session). `maxSends` stops
 * after that many sends — used by the kill-the-crank drill.
 */
export async function settlePlan(
  ctx: Ctx,
  session: Address,
  operator: TransactionSigner,
  plan: FillPlan[],
  opts: { maxSends?: number } = {},
): Promise<Outcome[]> {
  const s = await loadSession(ctx.rpc, session);
  const [config, usdcProgram, assetProgram] = await Promise.all([
    configPda(),
    tokenProgramOf(ctx.rpc, s.usdc_mint),
    tokenProgramOf(ctx.rpc, s.asset_mint),
  ]);
  const out: Outcome[] = [];
  let sends = 0;
  for (const f of [...plan].sort((a, b) => a.index - b.index)) {
    const target = `fill #${f.index}`;
    const receipt = await receiptPda(session, f.index);
    if (await exists(ctx.rpc, receipt)) {
      out.push({ target, result: "already-done", detail: "receipt exists" });
      continue;
    }
    if (opts.maxSends !== undefined && sends >= opts.maxSends) {
      out.push({ target, result: "skipped", detail: "crank stopped (maxSends)" });
      continue;
    }
    const [buyerEscrow, sellerEscrow] = await Promise.all([escrowPda(session, f.buyer), escrowPda(session, f.seller)]);
    // The engine may name the exact accounts; the program re-checks that
    // they belong to the seller / buyer either way.
    const [sellerUsdc, buyerAsset] = await Promise.all([
      f.sellerUsdc ?? ownerTokenAccount(ctx.rpc, f.seller, s.usdc_mint),
      f.buyerAsset ?? ownerTokenAccount(ctx.rpc, f.buyer, s.asset_mint),
    ]);
    if (!sellerUsdc || !buyerAsset) {
      out.push({ target, result: "skipped", detail: "seller USDC or buyer asset account missing" });
      continue;
    }
    const instruction = ix(
      "settle_fill",
      {
        config,
        session,
        operator,
        receipt,
        buyer_escrow: buyerEscrow,
        buyer_vault: await vaultPda(buyerEscrow),
        seller_escrow: sellerEscrow,
        seller_asset_vault: await assetVaultPda(sellerEscrow),
        seller_usdc: sellerUsdc,
        buyer_asset: buyerAsset,
        fee_receiver: s.fee_receiver,
        reserve: s.reserve,
        usdc_mint: s.usdc_mint,
        asset_mint: s.asset_mint,
        token_program: usdcProgram,
        asset_token_program: assetProgram,
      },
      {
        fill_index: f.index,
        qty: f.qty,
        price: f.price,
        disclosure_hash: f.disclosureHash ?? new Uint8Array(32),
      },
    );
    sends++;
    const r = await send(ctx, `settle ${target}`, [instruction], { cuLimit: 200_000 });
    if (r.status === "sent") out.push({ target, result: "sent", signature: r.signature });
    else if (r.alreadyInUse || (await exists(ctx.rpc, receipt))) out.push({ target, result: "already-done", detail: "landed concurrently" });
    else out.push({ target, result: "failed", detail: r.name ?? r.message });
  }
  return out;
}

/** Operator closes settlement so remaining escrow is refundable at once. */
export async function finish(ctx: Ctx, session: Address, operator: TransactionSigner): Promise<Outcome> {
  const s = await loadSession(ctx.rpc, session);
  if (s.status !== "Open") return { target: "finish", result: "already-done", detail: s.status };
  const r = await send(ctx, "finish_session", [ix("finish_session", { session, operator })], { cuLimit: 50_000 });
  return r.status === "sent"
    ? { target: "finish", result: "sent", signature: r.signature }
    : { target: "finish", result: "failed", detail: r.name ?? r.message };
}
