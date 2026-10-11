/**
 * The authority map (SRS 08 §8: "here is every key and what it can and
 * cannot do"), read from the cluster: each program's upgrade authority from
 * its ProgramData, and the authority of every singleton account the
 * programs administer.
 */
import { getAddressDecoder, getAddressEncoder, getProgramDerivedAddress, address, type Address } from "@solana/kit";
import type { Struct } from "./borsh";
import { envAddress } from "./addresses";
import { codecs, NPCS_MINT, pdas, PROGRAMS } from "./programs";
import { getAccounts, getProgramAccounts } from "./rpc";

export const LOADER_V3 = address("BPFLoaderUpgradeab1e11111111111111111111111");
/** The Squads v4 vault holding upgrade authority (`ops/squads/deployment-devnet.json`). */
export const SQUADS_VAULT = envAddress("squadsVault");

export interface ProgramRow {
  name: keyof typeof PROGRAMS;
  id: Address;
  deployed: boolean;
  /** null = immutable (or not deployed). */
  upgradeAuthority: Address | null;
  slot: bigint | null;
}

/** Loader v3 ProgramData: u32 variant (3), u64 slot, Option<Pubkey> authority. */
export function programDataAuthority(data: Uint8Array): { slot: bigint; authority: Address | null } {
  if (data.length < 13 || data[0] !== 3) throw new Error("not a ProgramData account");
  const slot = new DataView(data.buffer, data.byteOffset + 4, 8).getBigUint64(0, true);
  const authority = data[12] === 1 ? getAddressDecoder().decode(data.subarray(13, 45)) : null;
  return { slot, authority };
}

export interface AdminState {
  programs: ProgramRow[];
  rails: Struct | null;
  railsConfig: Address;
  gateMarket: Struct | null;
  gateRoot: Struct | null;
  feed: Struct | null;
  feedAddress: Address;
  venue: Struct | null;
  queue: Struct | null;
  grants: { address: Address; engine: Address; roles: number }[];
  auctionOperator: Address;
}

const decode = (c: (typeof codecs)[keyof typeof codecs], name: string, owner: Address, r: { owner: Address; data: Uint8Array } | null) => {
  if (!r || r.owner !== owner) return null;
  try {
    return c.decodeAccount(name, r.data);
  } catch {
    return null;
  }
};

export async function loadAdmin(mint: Address = NPCS_MINT): Promise<AdminState> {
  const names = Object.keys(PROGRAMS) as (keyof typeof PROGRAMS)[];
  const enc = getAddressEncoder();
  const pdAddrs = await Promise.all(
    names.map(async (n) => (await getProgramDerivedAddress({ programAddress: LOADER_V3, seeds: [enc.encode(PROGRAMS[n])] }))[0]),
  );
  const [railsConfig, gateMarketA, gateRootA, feedA, venueA, queueA, auctionOperator] = await Promise.all([
    pdas.railsConfig(),
    pdas.gateConfig(mint),
    pdas.gateRoot(),
    pdas.feed(mint),
    pdas.venue(),
    pdas.queue(mint),
    pdas.operator(),
  ]);
  const singles = [railsConfig, gateMarketA, gateRootA, feedA, venueA, queueA];
  const accs = await getAccounts([...names.map((n) => PROGRAMS[n]), ...pdAddrs, ...singles]);
  const progAcc = accs.slice(0, names.length);
  const pdAcc = accs.slice(names.length, 2 * names.length);
  const [railsR, gateR, rootR, feedR, venueR, queueR] = accs.slice(2 * names.length);
  const grants = await getProgramAccounts(PROGRAMS.rails, codecs.rails.discriminator("EngineGrant")).catch(() => []);
  return {
    programs: names.map((name, i) => {
      const pd = pdAcc[i];
      const parsed = pd ? programDataAuthority(pd.data) : null;
      return { name, id: PROGRAMS[name], deployed: !!progAcc[i], upgradeAuthority: parsed?.authority ?? null, slot: parsed?.slot ?? null };
    }),
    rails: decode(codecs.rails, "RailsConfig", PROGRAMS.rails, railsR ?? null),
    railsConfig,
    gateMarket: decode(codecs.gate, "MarketConfig", PROGRAMS.gate, gateR ?? null),
    gateRoot: decode(codecs.gate, "GateRoot", PROGRAMS.gate, rootR ?? null),
    feed: decode(codecs.mark, "Feed", PROGRAMS.mark, feedR ?? null),
    feedAddress: feedA,
    venue: decode(codecs.auction, "Venue", PROGRAMS.auction, venueR ?? null),
    queue: decode(codecs.queue, "Queue", PROGRAMS.queue, queueR ?? null),
    grants: grants.map((g) => {
      const s = codecs.rails.decodeAccount("EngineGrant", g.data);
      return { address: g.address, engine: s.engine as Address, roles: s.roles as number };
    }),
    auctionOperator,
  };
}
