/**
 * What the write pages (/queue, /facility, /tranches, /treasury) read
 * before offering a write: whether the program is on this cluster, and
 * whether the account the write needs (queue, facility, pool, treasury)
 * exists. Any of them may be missing on a cluster: the pages say so instead
 * of offering a write that cannot land.
 */
import type { Address } from "@solana/kit";
import type { Struct } from "./borsh";
import { codecs, pdas, PROGRAMS } from "./programs";
import { getAccount, getAccounts, getProgramAccounts } from "./rpc";

/** An executable program account at `program` (null: not deployed here). */
export async function deployed(program: Address): Promise<boolean> {
  return (await getAccount(program)) !== null;
}

const safe = <T>(fn: () => T): T | null => {
  try {
    return fn();
  } catch {
    return null;
  }
};

/** Mint facts a write needs: decimals and which token program owns it. */
export interface MintInfo {
  address: Address;
  decimals: number;
  program: Address;
  supply: bigint;
}

export async function mintInfos(mints: Address[]): Promise<(MintInfo | null)[]> {
  const raws = await getAccounts(mints);
  return raws.map((r, i) =>
    r && r.data.length >= 45
      ? {
          address: mints[i]!,
          decimals: r.data[44]!,
          program: r.owner,
          supply: new DataView(r.data.buffer, r.data.byteOffset + 36, 8).getBigUint64(0, true),
        }
      : null,
  );
}

// ── queue-claim ─────────────────────────────────────────────────────────

export interface QueueState {
  deployed: boolean;
  address: Address;
  queue: Struct | null;
  requests: { address: Address; request: Struct }[];
}

const REQUEST_HOLDER_OFFSET = 8 + 32 + 8; // queue, id

export async function loadQueue(mint: Address, holder: Address | null): Promise<QueueState> {
  const address = await pdas.queue(mint);
  if (!(await deployed(PROGRAMS.queue))) return { deployed: false, address, queue: null, requests: [] };
  const raw = await getAccount(address);
  const queue = raw && raw.owner === PROGRAMS.queue ? safe(() => codecs.queue.decodeAccount("Queue", raw.data)) : null;
  const requests = holder
    ? (
        await getProgramAccounts(PROGRAMS.queue, codecs.queue.discriminator("Request"), [{ offset: REQUEST_HOLDER_OFFSET, bytes: holder }])
      ).map((r) => ({ address: r.address, request: codecs.queue.decodeAccount("Request", r.data) }))
    : [];
  requests.sort((a, b) => ((a.request.id as bigint) < (b.request.id as bigint) ? 1 : -1));
  return { deployed: true, address, queue, requests };
}

// ── liquidity-facility ──────────────────────────────────────────────────

export interface FacilityRow {
  address: Address;
  config: Struct;
  /** payment-rails `Facility` (the fund), if open. */
  fund: Struct | null;
}

export interface BorrowerView {
  position: Address;
  state: Struct | null;
  /** payment-rails `Advance`: principal and interest at the last touch (null: none / repaid). */
  advance: Struct | null;
}

export async function loadFacilities(): Promise<{ deployed: boolean; facilities: FacilityRow[] }> {
  if (!(await deployed(PROGRAMS.facility))) return { deployed: false, facilities: [] };
  const raws = await getProgramAccounts(PROGRAMS.facility, codecs.facility.discriminator("FacilityConfig"));
  const configs = raws.map((r) => ({ address: r.address, config: codecs.facility.decodeAccount("FacilityConfig", r.data) }));
  const funds = await getAccounts(configs.map((c) => c.config.rails_facility as Address));
  return {
    deployed: true,
    facilities: configs
      .map((c, i) => ({
        ...c,
        fund: funds[i]?.owner === PROGRAMS.rails ? safe(() => codecs.rails.decodeAccount("Facility", funds[i]!.data)) : null,
      }))
      .sort((a, b) => ((a.config.facility_id as bigint) < (b.config.facility_id as bigint) ? -1 : 1)),
  };
}

/** One borrower's position in facility `config` and its rails advance. */
export async function loadBorrower(config: Address, railsFacility: Address, borrower: Address): Promise<BorrowerView> {
  const position = await pdas.position(config, borrower);
  const advanceA = await pdas.advance(railsFacility, borrower);
  const [p, adv] = await getAccounts([position, advanceA]);
  return {
    position,
    state: p?.owner === PROGRAMS.facility ? safe(() => codecs.facility.decodeAccount("Position", p.data)) : null,
    advance: adv?.owner === PROGRAMS.rails ? safe(() => codecs.rails.decodeAccount("Advance", adv.data)) : null,
  };
}

/** The borrower's rails payout destination: advances go to its registered token account only. */
export async function loadPayout(owner: Address): Promise<Struct | null> {
  const r = await getAccount(await pdas.payout(owner));
  return r?.owner === PROGRAMS.rails ? safe(() => codecs.rails.decodeAccount("PayoutDestination", r.data)) : null;
}

// ── tranche-engine ──────────────────────────────────────────────────────

export async function loadPools(): Promise<{ deployed: boolean; pools: { address: Address; pool: Struct }[] }> {
  if (!(await deployed(PROGRAMS.tranche))) return { deployed: false, pools: [] };
  const raws = await getProgramAccounts(PROGRAMS.tranche, codecs.tranche.discriminator("Pool"));
  return {
    deployed: true,
    pools: raws
      .map((r) => ({ address: r.address, pool: codecs.tranche.decodeAccount("Pool", r.data) }))
      .sort((a, b) => (a.pool.pool_id as number) - (b.pool.pool_id as number)),
  };
}

// ── npcs-treasury ───────────────────────────────────────────────────────

export interface TreasuryState {
  deployed: boolean;
  address: Address;
  treasury: Struct | null;
  vaultAmount: bigint | null;
}

/** SPL token account `amount`: the u64 at byte 64. */
export const tokenAmount = (data: Uint8Array) => new DataView(data.buffer, data.byteOffset + 64, 8).getBigUint64(0, true);

export async function loadTreasury(mint: Address): Promise<TreasuryState> {
  const address = await pdas.treasury(mint);
  if (!(await deployed(PROGRAMS.treasury))) return { deployed: false, address, treasury: null, vaultAmount: null };
  const raw = await getAccount(address);
  const treasury = raw?.owner === PROGRAMS.treasury ? safe(() => codecs.treasury.decodeAccount("Treasury", raw.data)) : null;
  if (!treasury) return { deployed: true, address, treasury: null, vaultAmount: null };
  const vault = await getAccount(treasury.vault as Address);
  return { deployed: true, address, treasury, vaultAmount: vault && vault.data.length >= 72 ? tokenAmount(vault.data) : null };
}
