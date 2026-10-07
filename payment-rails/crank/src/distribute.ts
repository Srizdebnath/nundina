/**
 * Distribution crank (SRS 08 §6 crank-runner, TE-P4, #141): pay one coupon
 * period of a tranche-engine pool through the rails waterfall.
 *
 * `tranche-engine::distribute(period, cash)` is permissionless: whoever
 * holds the period's actual cash (the servicer, in the pilot) pays it in,
 * and the program runs fees → senior coupon → reserve → senior amortisation
 * → junior coupon → junior residual through rails `distribute_coupon`,
 * paying the payees' registered destinations. The crank reads the pool,
 * derives every account (the rails `DistributeCoupon` list, forwarded, plus
 * the tranche signer's `EngineGrant`) and sends it. Like the other cranks it
 * keeps no state: the next period is the pool's own `periods` counter.
 */
import {
  AccountRole,
  getAddressDecoder,
  getAddressEncoder,
  getProgramDerivedAddress,
  type Address,
  type TransactionSigner,
} from "@solana/kit";
import { clockNow, fetchDecoded, ownerTokenAccount, rawAccount, tokenProgramOf } from "./chain.ts";
import type { Outcome } from "./crank.ts";
import { loadIdl, makeCodec, PROGRAM_ID as RAILS, type Struct } from "./idl.ts";
import { configPda, couponPda, destinationPda } from "./pda.ts";
import { send, type Ctx } from "./send.ts";

export const tranche = makeCodec(loadIdl("tranche_engine"));
export const TRANCHE = tranche.programId;
const SYSTEM = "11111111111111111111111111111111" as Address;

export interface Pool extends Struct {
  pool_id: number;
  params: Struct & { senior_payee_token: Address; junior_payee_token: Address };
  cash: Address;
  state: string;
  periods: number;
  last_distribution: bigint;
}

/** `tranche_engine::rails_period_id`: pool id in the high word, period in the low. */
export function railsPeriodId(poolId: number, period: number): bigint {
  return (BigInt(poolId) << 32n) | BigInt(period);
}

/**
 * Pure: is a distribution due? The program accepts one whenever the pool is
 * Active or Amortizing; the runner adds a minimum interval so a period is a
 * real period, not every tick.
 */
export function distributionDue(p: Pool, now: bigint, minIntervalSeconds: bigint): { period: number } | null {
  if (p.state !== "Active" && p.state !== "Amortizing") return null;
  if (now - p.last_distribution < minIntervalSeconds) return null;
  return { period: p.periods };
}

async function signerPda(): Promise<Address> {
  const [a] = await getProgramDerivedAddress({ programAddress: TRANCHE, seeds: [new TextEncoder().encode("te_signer")] });
  return a;
}

async function grantPda(engine: Address): Promise<Address> {
  const [a] = await getProgramDerivedAddress({
    programAddress: RAILS,
    seeds: [new TextEncoder().encode("engine"), getAddressEncoder().encode(engine)],
  });
  return a;
}

export async function loadPool(ctx: Ctx, pool: Address): Promise<Pool> {
  const raw = await rawAccount(ctx.rpc, pool);
  const p = raw && raw.owner === TRANCHE ? tranche.decodeAccount<Pool>("Pool", raw.data) : null;
  if (!p) throw new Error(`${pool} is not a tranche-engine pool`);
  return p;
}

/** Owner of an SPL / Token-2022 account (bytes 32..64). */
async function tokenOwner(ctx: Ctx, account: Address): Promise<Address> {
  const raw = await rawAccount(ctx.rpc, account);
  if (!raw || raw.data.length < 64) throw new Error(`token account ${account} missing`);
  return getAddressDecoder().decode(raw.data.subarray(32, 64));
}

/** The `distribute` instruction for `period`, every account derived from chain state. */
export async function distributeIx(ctx: Ctx, pool: Address, p: Pool, period: number, cash: bigint, payer: TransactionSigner) {
  const cfg = await fetchDecoded<Struct & { usdc_mint: Address; fee_receiver: Address; reserve: Address }>(
    ctx.rpc,
    "RailsConfig",
    await configPda(),
  );
  if (!cfg) throw new Error("rails config missing");
  const usdc = cfg.usdc_mint;
  const payerUsdc = await ownerTokenAccount(ctx.rpc, payer.address, usdc);
  if (!payerUsdc) throw new Error(`${payer.address} has no USDC account for ${usdc}`);
  const [tokenProgram, signer, seniorOwner, juniorOwner] = await Promise.all([
    tokenProgramOf(ctx.rpc, usdc),
    signerPda(),
    tokenOwner(ctx, p.params.senior_payee_token),
    tokenOwner(ctx, p.params.junior_payee_token),
  ]);
  const ro = (address: Address) => ({ address, role: AccountRole.READONLY });
  const w = (address: Address) => ({ address, role: AccountRole.WRITABLE });
  const base = tranche.ix(
    "distribute",
    {
      pool,
      cash: p.cash,
      payer,
      payer_usdc: payerUsdc,
      usdc_mint: usdc,
      signer,
      token_program: tokenProgram,
      rails_program: RAILS,
      system_program: SYSTEM,
    },
    { period, cash },
  );
  // Rails `DistributeCoupon`, forwarded: the tranche signer PDA is the
  // operator (the engine signs for it), then its EngineGrant.
  const rails = [
    ro(await configPda()),
    w(await couponPda(signer, railsPeriodId(p.pool_id, period))),
    w(p.cash),
    w(cfg.fee_receiver),
    w(cfg.reserve),
    ro(await destinationPda(seniorOwner)),
    w(p.params.senior_payee_token),
    ro(await destinationPda(juniorOwner)),
    w(p.params.junior_payee_token),
    ro(usdc),
    w(signer),
    ro(tokenProgram),
    ro(SYSTEM),
    ro(await grantPda(signer)),
  ];
  return { ...base, accounts: [...(base.accounts ?? []), ...rails] };
}

/** Pay the next period of `pool` with `cash` micro-USDC from the crank wallet. */
export async function distribute(ctx: Ctx, pool: Address, cash: bigint, payer: TransactionSigner): Promise<Outcome> {
  const p = await loadPool(ctx, pool);
  const due = distributionDue(p, await clockNow(ctx.rpc), 0n);
  const target = `pool ${pool} period ${p.periods}`;
  if (!due) return { target, result: "skipped", detail: `pool is ${p.state}` };
  const ix = await distributeIx(ctx, pool, p, due.period, cash, payer);
  const r = await send(ctx, `distribute ${target}`, [ix]);
  return r.status === "sent"
    ? { target, result: "sent", signature: r.signature }
    : { target, result: "failed", detail: r.name ?? r.message };
}
