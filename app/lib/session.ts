/**
 * auction-engine sessions (#136) as the session page shows them: phase
 * clock, sealed and revealed orders, the uniform clearing price against the
 * NAV the reserve was set from, fills and their DvP settlement, the print.
 */
import type { Address } from "@solana/kit";
import type { Struct } from "./borsh";
import { codecs, pdas, PROGRAMS } from "./programs";
import { getAccount, getAccounts, getProgramAccounts } from "./rpc";

export const PHASES = ["Commit", "Reveal", "Allocating", "IssuerRofr", "Settling", "Printed"] as const;

/** NAV scale: micro-USDC per base unit × 10¹² (mark-engine `nav_pico`). */
export const PICO = 1_000_000_000_000n;

export interface OrderView {
  owner: Address;
  side: "Buy" | "Sell";
  commitment: Uint8Array;
  escrowed: bigint;
  revealed: boolean;
  limit: bigint;
  qty: bigint;
  alloc: bigint;
}

export interface FillView {
  index: number;
  buyer: Address;
  seller: Address;
  qty: bigint;
  rofr: boolean;
  settled: boolean;
}

export interface SessionView {
  address: Address;
  id: bigint;
  phase: string;
  assetMint: Address;
  assetDecimals: number;
  feed: Address;
  railsSession: Address;
  params: Struct;
  reservePrice: bigint;
  referenceMark: bigint;
  clearPrice: bigint;
  clearedVolume: bigint;
  rofrQty: bigint;
  fillsSettled: number;
  orders: OrderView[];
  fills: FillView[];
  feedState: Struct | null;
  print: Struct | null;
  rails: Struct | null;
}

export function toSessionView(address: Address, s: Struct): Omit<SessionView, "feedState" | "print" | "rails"> {
  return {
    address,
    id: s.session_id as bigint,
    phase: s.phase as string,
    assetMint: s.asset_mint as Address,
    assetDecimals: s.asset_decimals as number,
    feed: s.feed as Address,
    railsSession: s.rails_session as Address,
    params: s.params as Struct,
    reservePrice: s.reserve_price as bigint,
    referenceMark: s.reference_mark as bigint,
    clearPrice: s.clear_price as bigint,
    clearedVolume: s.cleared_volume as bigint,
    rofrQty: s.rofr_qty as bigint,
    fillsSettled: s.fills_settled as number,
    orders: (s.orders as Struct[]).map((o) => ({
      owner: o.owner as Address,
      side: o.side as "Buy" | "Sell",
      commitment: o.commitment as Uint8Array,
      escrowed: o.escrowed as bigint,
      revealed: o.revealed as boolean,
      limit: o.limit as bigint,
      qty: o.qty as bigint,
      alloc: o.alloc as bigint,
    })),
    fills: (s.fills as Struct[]).map((f, index) => ({
      index,
      buyer: f.buyer as Address,
      seller: f.seller as Address,
      qty: f.qty as bigint,
      rofr: f.rofr as boolean,
      settled: f.settled as boolean,
    })),
  };
}

export async function loadSession(address: Address): Promise<SessionView | null> {
  const raw = await getAccount(address);
  if (!raw || raw.owner !== PROGRAMS.auction) return null;
  let s: Struct;
  try {
    s = codecs.auction.decodeAccount("Session", raw.data);
  } catch {
    return null;
  }
  const base = toSessionView(address, s);
  const printA = await pdas.printRecord(base.feed, address);
  const [feedR, printR, railsR] = await getAccounts([base.feed, printA, base.railsSession]);
  const safe = (fn: () => Struct): Struct | null => {
    try {
      return fn();
    } catch {
      return null;
    }
  };
  return {
    ...base,
    feedState: feedR ? safe(() => codecs.mark.decodeAccount("Feed", feedR.data)) : null,
    print: printR ? safe(() => codecs.mark.decodeAccount("PrintRecord", printR.data)) : null,
    rails: railsR ? safe(() => codecs.rails.decodeAccount("CashSession", railsR.data)) : null,
  };
}

/** Every session the venue has opened, newest first. */
export async function listSessions(): Promise<ReturnType<typeof toSessionView>[]> {
  const raws = await getProgramAccounts(PROGRAMS.auction, codecs.auction.discriminator("Session"));
  return raws
    .map((r) => toSessionView(r.address, codecs.auction.decodeAccount("Session", r.data)))
    .sort((a, b) => (a.id < b.id ? 1 : -1));
}

export interface CurvePoint {
  price: bigint;
  qty: bigint;
}

/**
 * Step curves from the revealed book: cumulative demand at or above each
 * bid limit, cumulative supply at or below each ask limit.
 */
export function curves(orders: OrderView[]): { demand: CurvePoint[]; supply: CurvePoint[] } {
  const rev = orders.filter((o) => o.revealed && o.qty > 0n);
  const bids = rev.filter((o) => o.side === "Buy").sort((a, b) => (a.limit > b.limit ? -1 : a.limit < b.limit ? 1 : 0));
  const asks = rev.filter((o) => o.side === "Sell").sort((a, b) => (a.limit < b.limit ? -1 : a.limit > b.limit ? 1 : 0));
  let d = 0n;
  const demand = bids.map((o) => ({ price: o.limit, qty: (d += o.qty) }));
  let s = 0n;
  const supply = asks.map((o) => ({ price: o.limit, qty: (s += o.qty) }));
  return { demand, supply };
}

/** Which phase the clock says the session should be in (cranks lag it). */
export function clockPhase(params: Struct, phase: string, now: number): string {
  if (phase === "Printed" || phase === "Failed") return phase;
  const commitEnd = Number(params.commit_end as bigint);
  const revealEnd = Number(params.reveal_end as bigint);
  const deadline = Number(params.settle_deadline as bigint);
  if (now < commitEnd) return "Commit";
  if (now < revealEnd) return "Reveal";
  if (now >= deadline) return "Expired";
  return phase;
}

/** Fixed-point micro-units (6 dp) as a decimal string. */
export function fmt6(v: bigint, dp = 6): string {
  const neg = v < 0n;
  const a = neg ? -v : v;
  const whole = a / 1_000_000n;
  const frac = (a % 1_000_000n).toString().padStart(6, "0").slice(0, dp);
  return `${neg ? "-" : ""}${whole.toLocaleString("en-US")}${dp > 0 ? "." + frac : ""}`;
}

/** NAV in pico units → price per whole unit in USDC, 6 dp. */
export function navToPrice(navPico: bigint, assetDecimals: number): bigint {
  // nav_pico = micro-USDC per base unit × 10¹²; a whole unit is 10^dec base units.
  return (navPico * 10n ** BigInt(assetDecimals)) / PICO;
}

/**
 * USDC for `qty` base units at `price` micro-USDC per whole unit, rounded UP
 * (auction-engine `gross`, payment-rails escrow math). Escrowing the floored value
 * under-funds by one base unit whenever `price × qty` is not a multiple of
 * 10^decimals, and `reveal` then fails with `BidExceedsEscrow`.
 */
export function gross(qty: bigint, price: bigint, assetDecimals: number): bigint {
  const scale = 10n ** BigInt(assetDecimals);
  return (qty * price + scale - 1n) / scale;
}

/** "0.95" → 950000n at `dp` decimals; rejects more precision than `dp`. */
export function parseUnits(s: string, dp: number): bigint {
  const m = s.trim().match(/^(\d+)(?:\.(\d+))?$/);
  if (!m) throw new Error(`not a number: ${s}`);
  const frac = m[2] ?? "";
  if (frac.length > dp) throw new Error(`at most ${dp} decimals: ${s}`);
  return BigInt(m[1]!) * 10n ** BigInt(dp) + BigInt(frac.padEnd(dp, "0") || "0");
}
