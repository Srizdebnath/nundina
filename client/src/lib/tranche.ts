/**
 * Tranche positions (TE, #140): a wallet's senior / junior tokens of each
 * tranche-engine pool, as a share of that tranche's supply and so of its
 * principal and allocated losses (the pool's three books).
 */
import type { Address } from "@solana/kit";
import type { Struct } from "./borsh";
import { codecs, PROGRAMS } from "./programs";
import { getAccounts, getProgramAccounts, type TokenHolding } from "./rpc";

export interface TranchePosition {
  pool: Address;
  poolId: number;
  state: string;
  side: "senior" | "junior";
  mint: Address;
  amount: bigint;
  supply: bigint;
  /** Share of the tranche in basis points (floor). */
  shareBps: bigint;
  /** This holding's part of the tranche's principal and of its realised loss (floor). */
  principal: bigint;
  loss: bigint;
  /** Junior units are frozen while locked up (TE-P2). */
  locked: boolean;
  couponBps: number;
}

/** SPL / Token-2022 mint: `supply` is the u64 at byte 36. */
export function mintSupply(data: Uint8Array): bigint {
  if (data.length < 44) throw new Error("not a mint");
  return new DataView(data.buffer, data.byteOffset + 36, 8).getBigUint64(0, true);
}

const share = (x: bigint, amount: bigint, supply: bigint) => (supply === 0n ? 0n : (x * amount) / supply);

/** Pure: one holding of one pool's tranche mint. */
export function tranchePosition(pool: Address, p: Struct, h: TokenHolding, supply: bigint): TranchePosition | null {
  const side = h.mint === p.senior_mint ? "senior" : h.mint === p.junior_mint ? "junior" : null;
  if (!side || h.amount === 0n) return null;
  const params = p.params as Struct;
  const principal = (side === "senior" ? p.senior_principal : p.junior_principal) as bigint;
  const loss = (side === "senior" ? p.senior_loss : p.junior_loss) as bigint;
  return {
    pool,
    poolId: p.pool_id as number,
    state: typeof p.state === "string" ? p.state : Object.keys(p.state as Struct)[0] ?? "?",
    side,
    mint: h.mint,
    amount: h.amount,
    supply,
    shareBps: share(10_000n, h.amount, supply),
    principal: share(principal, h.amount, supply),
    loss: share(loss, h.amount, supply),
    locked: h.frozen,
    couponBps: Number((side === "senior" ? params.senior_coupon_bps : params.junior_coupon_bps) as number),
  };
}

export async function loadTranchePositions(holdings: TokenHolding[]): Promise<TranchePosition[]> {
  const pools = await getProgramAccounts(PROGRAMS.tranche, codecs.tranche.discriminator("Pool"));
  const decoded = pools.map((r) => ({ address: r.address, p: codecs.tranche.decodeAccount("Pool", r.data) }));
  const held = decoded.flatMap(({ address, p }) =>
    holdings.filter((h) => h.mint === p.senior_mint || h.mint === p.junior_mint).map((h) => ({ address, p, h })),
  );
  if (held.length === 0) return [];
  const mints = await getAccounts(held.map(({ h }) => h.mint));
  return held
    .map(({ address, p, h }, i) => tranchePosition(address, p, h, mints[i] ? mintSupply(mints[i]!.data) : 0n))
    .filter((x): x is TranchePosition => x !== null);
}
