/**
 * The portfolio page (#144): one wallet's NPCS, its seasoning lots (CG-P3),
 * its redemption-queue claims (#138), facility advances (#139) and its
 * orders in auction sessions (#136).
 */
import type { Address } from "@solana/kit";
import type { Struct } from "./borsh";
import { lotsView, type LotView } from "./gate";
import { codecs, pdas, PROGRAMS } from "./programs";
import { getAccounts, getProgramAccounts, getTokenHoldings, type TokenHolding } from "./rpc";
import { listSessions } from "./session";
import { loadTranchePositions, type TranchePosition } from "./tranche";

export interface Portfolio {
  wallet: Address;
  mint: Address;
  holdings: TokenHolding[];
  link: Struct | null;
  lots: { freeQty: bigint; lots: LotView[] } | null;
  claims: { address: Address; request: Struct }[];
  positions: { address: Address; position: Struct }[];
  tranches: TranchePosition[];
  orders: { session: Address; sessionId: bigint; phase: string; side: string; escrowed: bigint; revealed: boolean; limit: bigint; qty: bigint; alloc: bigint }[];
}

// Offsets after the 8-byte discriminator (IDL field order).
const REQUEST_HOLDER_OFFSET = 8 + 32 + 8; // queue, id
const POSITION_BORROWER_OFFSET = 8 + 32; // config

export async function loadPortfolio(wallet: Address, mint: Address): Promise<Portfolio> {
  const [linkA, lotsA] = await Promise.all([pdas.holderLink(mint, wallet), pdas.lots(mint, wallet)]);
  const [holdings, [linkR, lotsR], requests, positions, sessions] = await Promise.all([
    getTokenHoldings(wallet),
    getAccounts([linkA, lotsA]),
    getProgramAccounts(PROGRAMS.queue, codecs.queue.discriminator("Request"), [
      { offset: REQUEST_HOLDER_OFFSET, bytes: wallet },
    ]).catch(() => []),
    getProgramAccounts(PROGRAMS.facility, codecs.facility.discriminator("Position"), [
      { offset: POSITION_BORROWER_OFFSET, bytes: wallet },
    ]).catch(() => []),
    listSessions().catch(() => []),
  ]);
  const gate = (r: typeof linkR, name: string) =>
    r && r.owner === PROGRAMS.gate ? codecs.gate.decodeAccount(name, r.data) : null;
  const lots = gate(lotsR, "HolderLots");
  const tranches = await loadTranchePositions(holdings).catch(() => []);
  return {
    wallet,
    mint,
    holdings,
    link: gate(linkR, "HolderLink"),
    lots: lots ? lotsView(lots) : null,
    claims: requests.map((r) => ({ address: r.address, request: codecs.queue.decodeAccount("Request", r.data) })),
    positions: positions.map((r) => ({ address: r.address, position: codecs.facility.decodeAccount("Position", r.data) })),
    tranches,
    orders: sessions.flatMap((s) =>
      s.orders
        .filter((o) => o.owner === wallet)
        .map((o) => ({
          session: s.address,
          sessionId: s.id,
          phase: s.phase,
          side: o.side,
          escrowed: o.escrowed,
          revealed: o.revealed,
          limit: o.limit,
          qty: o.qty,
          alloc: o.alloc,
        })),
    ),
  };
}
