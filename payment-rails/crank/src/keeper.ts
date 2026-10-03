/**
 * Keeper side of the crank (doc 06 §8, #127). The keeper is `ctx.payer`
 * (KEYPAIR): a wallet of its own, bonded on-chain, not the operator's.
 *
 *   register  — post the bond (≥ policy min) into the keeper's PDA vault
 *   top-up    — add to it
 *   assign    — the session operator assigns a keeper (operator signs)
 *   release   — anyone, once the session has no open escrow
 *   withdraw  — the keeper takes the whole bond back (no live assignment)
 *
 * Inside the SLA the keeper refunds with the ordinary `refunds` command;
 * past it, anyone running `refunds --escalate-to <usdc>` is paid from the
 * keeper's bond (crank.ts `refundAll`).
 */
import type { Address, TransactionSigner } from "@solana/kit";
import { fetchDecoded, tokenProgramOf } from "./chain.ts";
import type { CashSession, Keeper, Outcome } from "./crank.ts";
import { ix, type Struct } from "./idl.ts";
import { configPda, keeperPda, keeperPolicyPda, keeperVaultPda, sessionKeeperPda } from "./pda.ts";
import { send, type Ctx } from "./send.ts";

interface RailsConfig extends Struct {
  usdc_mint: Address;
}

async function outcome(ctx: Ctx, target: string, label: string, instruction: ReturnType<typeof ix>): Promise<Outcome> {
  const r = await send(ctx, label, [instruction], { cuLimit: 120_000 });
  return r.status === "sent"
    ? { target, result: "sent", signature: r.signature }
    : { target, result: "failed", detail: r.name ?? r.message };
}

export async function registerKeeper(ctx: Ctx, bond: bigint, keeperToken: Address): Promise<Outcome> {
  const config = await configPda();
  const c = await fetchDecoded<RailsConfig>(ctx.rpc, "RailsConfig", config);
  if (!c) throw new Error("rails config not found");
  const keeper = await keeperPda(ctx.payer.address);
  return outcome(ctx, `keeper ${ctx.payer.address}`, `register keeper (bond ${bond})`, ix("register_keeper", {
    config,
    policy: await keeperPolicyPda(),
    keeper,
    vault: await keeperVaultPda(keeper),
    usdc_mint: c.usdc_mint,
    keeper_token: keeperToken,
    wallet: ctx.payer,
    token_program: await tokenProgramOf(ctx.rpc, c.usdc_mint),
  }, { bond }));
}

export async function topUpKeeper(ctx: Ctx, amount: bigint, keeperToken: Address): Promise<Outcome> {
  const keeper = await keeperPda(ctx.payer.address);
  const k = await fetchDecoded<Keeper & { vault: Address }>(ctx.rpc, "Keeper", keeper);
  if (!k) throw new Error("not a registered keeper");
  const c = await fetchDecoded<RailsConfig>(ctx.rpc, "RailsConfig", await configPda());
  return outcome(ctx, `keeper ${ctx.payer.address}`, `top up keeper (+${amount})`, ix("top_up_keeper", {
    keeper,
    vault: k.vault,
    usdc_mint: c!.usdc_mint,
    keeper_token: keeperToken,
    wallet: ctx.payer,
    token_program: await tokenProgramOf(ctx.rpc, c!.usdc_mint),
  }, { amount }));
}

export async function assignKeeper(ctx: Ctx, session: Address, operator: TransactionSigner, keeperWallet: Address): Promise<Outcome> {
  const keeper = await keeperPda(keeperWallet);
  return outcome(ctx, `session ${session}`, `assign keeper ${keeperWallet}`, ix("assign_keeper", {
    policy: await keeperPolicyPda(),
    session,
    keeper,
    vault: await keeperVaultPda(keeper),
    session_keeper: await sessionKeeperPda(session),
    operator,
  }));
}

export async function releaseAssignment(ctx: Ctx, session: Address): Promise<Outcome> {
  const s = await fetchDecoded<CashSession>(ctx.rpc, "CashSession", session);
  const sk = await fetchDecoded<{ keeper: Address } & Struct>(ctx.rpc, "SessionKeeper", await sessionKeeperPda(session));
  if (!s || !sk) return { target: `session ${session}`, result: "skipped", detail: "no keeper assignment" };
  return outcome(ctx, `session ${session}`, "release keeper assignment", ix("release_assignment", {
    session,
    session_keeper: await sessionKeeperPda(session),
    keeper: sk.keeper,
    operator: s.operator,
    caller: ctx.payer,
  }));
}

export async function withdrawKeeper(ctx: Ctx, keeperToken: Address): Promise<Outcome> {
  const keeper = await keeperPda(ctx.payer.address);
  const c = await fetchDecoded<RailsConfig>(ctx.rpc, "RailsConfig", await configPda());
  return outcome(ctx, `keeper ${ctx.payer.address}`, "withdraw keeper bond", ix("withdraw_keeper", {
    keeper,
    vault: await keeperVaultPda(keeper),
    usdc_mint: c!.usdc_mint,
    keeper_token: keeperToken,
    wallet: ctx.payer,
    token_program: await tokenProgramOf(ctx.rpc, c!.usdc_mint),
  }));
}
