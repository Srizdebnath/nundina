/**
 * The public program registry for the marketing pages: names and roles of the
 * eight programs, with ids from the environment (`lib/addresses.ts`) and the
 * deployed bit read live from the cluster.
 */
import { PROGRAMS } from "./programs";
import { getAccounts } from "./rpc";

export type ProgramKey = keyof typeof PROGRAMS;

export const REGISTRY: { key: ProgramKey; name: string; role: string; doc: string }[] = [
  { key: "rails", name: "payment-rails", role: "USDC cash layer: escrow, bonds, DvP settlement, refunds, ROFR, advances", doc: "06-payment-rails" },
  { key: "gate", name: "credit-gate", role: "Compliance at account activation: SAS reads, holder cap, lot ledger, seasoning, settle_check", doc: "07-credit-gate" },
  { key: "auction", name: "auction-engine", role: "Sealed-bid sessions: commit, reveal, uniform-price clear, pro-rata, print", doc: "08-pricing-engines" },
  { key: "mark", name: "mark-engine", role: "NAV publication, mark staleness states, auction prints", doc: "08-pricing-engines" },
  { key: "queue", name: "queue-claim", role: "Tokenized redemption-queue claims: request, attest, sell, pledge", doc: "09-queue-and-liquidity" },
  { key: "facility", name: "liquidity-facility", role: "Advances against claims and NPCS: health, stale-mark guard, liquidation ladder", doc: "09-queue-and-liquidity" },
  { key: "tranche", name: "tranche-engine", role: "Senior/junior pools, three books, coupon waterfall", doc: "10-tranching-treasury-math" },
  { key: "treasury", name: "npcs-treasury", role: "NPCS mint-on-deposit and redeem-at-NAV against the USDC sleeve", doc: "10-tranching-treasury-math" },
];

/** Which programs are deployed (owned by the upgradeable loader) on the configured cluster (null when the RPC is unreachable). */
export async function deployedMap(): Promise<Record<ProgramKey, boolean> | null> {
  try {
    const accts = await getAccounts(REGISTRY.map((r) => PROGRAMS[r.key]));
    return Object.fromEntries(REGISTRY.map((r, i) => [r.key, accts[i]?.owner === "BPFLoaderUpgradeab1e11111111111111111111111"])) as Record<ProgramKey, boolean>;
  } catch {
    return null;
  }
}
