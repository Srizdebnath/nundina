/**
 * Deployment addresses — read from the environment, never from source.
 *
 * Every program id, the pilot mint and the governance vault come from
 * `NEXT_PUBLIC_*` variables (`client/.env`, committed: public ids only;
 * `client/.env.local` overrides it locally; `.env.example` lists them all). A
 * redeploy is an env change, not a code change.
 *
 * Each variable is referenced by its literal name below because Next.js only
 * inlines `process.env.NEXT_PUBLIC_X` into the browser bundle when written out
 * in full.
 *
 * A missing or malformed value does not take the app down: it resolves to the
 * system program (which owns no program accounts, so reads come back empty)
 * and `MISSING_ENV` names it, so `DeploymentBanner` can say exactly which
 * variable to set.
 */
import { address, isAddress, type Address } from "@solana/kit";

const RAW = {
  auction: ["NEXT_PUBLIC_AUCTION_ENGINE_PROGRAM", process.env.NEXT_PUBLIC_AUCTION_ENGINE_PROGRAM],
  gate: ["NEXT_PUBLIC_CREDIT_GATE_PROGRAM", process.env.NEXT_PUBLIC_CREDIT_GATE_PROGRAM],
  mark: ["NEXT_PUBLIC_MARK_ENGINE_PROGRAM", process.env.NEXT_PUBLIC_MARK_ENGINE_PROGRAM],
  queue: ["NEXT_PUBLIC_QUEUE_CLAIM_PROGRAM", process.env.NEXT_PUBLIC_QUEUE_CLAIM_PROGRAM],
  facility: ["NEXT_PUBLIC_LIQUIDITY_FACILITY_PROGRAM", process.env.NEXT_PUBLIC_LIQUIDITY_FACILITY_PROGRAM],
  tranche: ["NEXT_PUBLIC_TRANCHE_ENGINE_PROGRAM", process.env.NEXT_PUBLIC_TRANCHE_ENGINE_PROGRAM],
  treasury: ["NEXT_PUBLIC_NPCS_TREASURY_PROGRAM", process.env.NEXT_PUBLIC_NPCS_TREASURY_PROGRAM],
  rails: ["NEXT_PUBLIC_PAYMENT_RAILS_PROGRAM", process.env.NEXT_PUBLIC_PAYMENT_RAILS_PROGRAM],
  sas: ["NEXT_PUBLIC_SAS_PROGRAM", process.env.NEXT_PUBLIC_SAS_PROGRAM],
  npcsMint: ["NEXT_PUBLIC_NPCS_MINT", process.env.NEXT_PUBLIC_NPCS_MINT],
  squadsVault: ["NEXT_PUBLIC_SQUADS_VAULT", process.env.NEXT_PUBLIC_SQUADS_VAULT],
} as const;

export type AddressKey = keyof typeof RAW;

/** The system program: an always-valid stand-in that owns no program accounts. */
const UNSET = address("11111111111111111111111111111111");

/** Env variable names that are missing or not base58 addresses. */
export const MISSING_ENV: string[] = Object.values(RAW)
  .filter(([, v]) => !v || !isAddress(v.trim()))
  .map(([name]) => name);

export function envAddress(key: AddressKey): Address {
  const v = RAW[key][1]?.trim();
  return v && isAddress(v) ? address(v) : UNSET;
}
