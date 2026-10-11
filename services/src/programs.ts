/**
 * The programs the services read, with their IDLs. One copy of the IDLs
 * lives in `client/src/idl/` (synced from `anchor build`, drift-checked by the app's
 * tests); the services read the same files.
 */
import { readFileSync } from "node:fs";
import { address, type Address } from "@solana/kit";
import { Codec, type Idl } from "./codec.ts";

const load = (name: string) =>
  new Codec(JSON.parse(readFileSync(new URL(`../../client/src/idl/${name}.json`, import.meta.url), "utf8")) as Idl);

export const PROGRAMS = {
  "payment-rails": load("payment_rails"),
  "credit-gate": load("credit_gate"),
  "auction-engine": load("auction_engine"),
  "mark-engine": load("mark_engine"),
  "queue-claim": load("queue_claim"),
  "liquidity-facility": load("liquidity_facility"),
  "tranche-engine": load("tranche_engine"),
  "npcs-treasury": load("npcs_treasury"),
} as const;
export type ProgramName = keyof typeof PROGRAMS;

export const BY_ID = new Map<Address, { name: ProgramName; codec: Codec }>(
  (Object.entries(PROGRAMS) as [ProgramName, Codec][]).map(([name, codec]) => [address(codec.idl.address), { name, codec }]),
);

export const programId = (name: ProgramName): Address => address(PROGRAMS[name].idl.address);
