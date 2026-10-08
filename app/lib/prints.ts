/**
 * Prints and their disclosure classes (ME-P6). mark-engine keeps every
 * `PrintRecord` in full and emits `PrintRecorded` with what the market's
 * class discloses: PUBLIC price + volume, BANDED the price rounded down to
 * the band + volume, AGGREGATE_ONLY volume only, DATA_ROOM neither.
 * Public views show that disclosure; `/data-room` (allowlisted) shows the
 * record. The account bytes are still readable by anyone with an RPC:
 * this is disclosure discipline in the product, not secrecy on-chain.
 */
import type { Address } from "@solana/kit";
import type { Struct } from "./borsh";
import { codecs, PROGRAMS } from "./programs";
import { getAccounts, getProgramAccounts } from "./rpc";

export type PrintPolicy = "Public" | "Banded" | "AggregateOnly" | "DataRoom";

const BPS = 10_000n;

/** mark-engine `band`: round down to a band `band_bps` of the price wide (at least one micro). */
export function band(price: bigint, bandBps: bigint): bigint {
  let width = (price * bandBps) / BPS;
  if (width < 1n) width = 1n;
  return (price / width) * width;
}

/** What a print discloses outside the data room (the `PrintRecorded` event). */
export function disclose(policy: PrintPolicy, price: bigint, volume: bigint, bandBps: bigint): { price: bigint | null; volume: bigint | null } {
  switch (policy) {
    case "Public":
      return { price, volume };
    case "Banded":
      return { price: band(price, bandBps), volume };
    case "AggregateOnly":
      return { price: null, volume };
    case "DataRoom":
      return { price: null, volume: null };
  }
}

export interface PrintRow {
  address: Address;
  feed: Address;
  session: Address;
  price: bigint;
  volume: bigint;
  investors: number;
  forcedSale: boolean;
  eligibility: string;
  recordedAt: number;
  policy: PrintPolicy;
  bandBps: bigint;
  /** Feed NAV per whole unit when loaded (micro-USDC), for the discount. */
  navPico: bigint;
  assetDecimals: number;
}

export const enumName = (v: unknown) => (typeof v === "string" ? v : Object.keys(v as Struct)[0] ?? "?");

/** Every print on the cluster with its feed's class, newest first. */
export async function loadPrints(): Promise<PrintRow[]> {
  const recs = await getProgramAccounts(PROGRAMS.mark, codecs.mark.discriminator("PrintRecord"));
  const decoded = recs.map((r) => ({ address: r.address, rec: codecs.mark.decodeAccount("PrintRecord", r.data) }));
  const feeds = [...new Set(decoded.map((d) => d.rec.feed as Address))];
  const feedAccs = await getAccounts(feeds);
  const feedBy = new Map(
    feeds.map((f, i) => [f, feedAccs[i] ? codecs.mark.decodeAccount("Feed", feedAccs[i]!.data) : null] as const),
  );
  return decoded
    .map(({ address, rec }) => {
      const f = feedBy.get(rec.feed as Address) ?? null;
      const params = (f?.params ?? {}) as Struct;
      return {
        address,
        feed: rec.feed as Address,
        session: rec.session as Address,
        price: rec.price as bigint,
        volume: rec.volume as bigint,
        investors: rec.n_investor_ids as number,
        forcedSale: rec.forced_sale as boolean,
        eligibility: enumName(rec.eligibility),
        recordedAt: Number(rec.recorded_at as bigint),
        // An unreadable feed discloses nothing (fail closed).
        policy: (f ? enumName(params.print_policy) : "DataRoom") as PrintPolicy,
        bandBps: (params.band_bps as bigint | undefined) ?? 0n,
        navPico: (f?.nav_pico as bigint | undefined) ?? 0n,
        assetDecimals: (f?.asset_decimals as number | undefined) ?? 6,
      };
    })
    .sort((a, b) => b.recordedAt - a.recordedAt);
}
