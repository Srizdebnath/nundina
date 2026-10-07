/**
 * NAV computation (SRS 08 §3.2, doc 09 §3 I2): the exact arithmetic of
 * `credit-math` (`programs/credit-math/src/nav.rs`), in bigint, so the
 * publisher computes what the crate computes. Every division floors: the
 * publisher initiates the mark, so rounding goes against it.
 *
 *   NAV_gross = floor(treasury_µUSDC × 10¹² / supply_base)          (I2a)
 *   fee       = floor(NAV_gross × fee_bps × Δt / (10⁴ × 31_536_000)) (I2b)
 *   NAV       = NAV_gross − fee                                       (I2c)
 *
 * The inputs go into a canonical JSON document whose SHA-256 is the
 * on-chain `inputs_hash`; the document itself is the evidence that lets
 * anyone recompute the published NAV.
 */
import { createHash } from "node:crypto";

export const PICO = 1_000_000_000_000n;
export const SECONDS_PER_YEAR = 31_536_000n;
/** Bumped whenever the computation below changes. */
export const MODEL_VERSION = 1;

export function navGrossPico(treasuryMicros: bigint, supplyBase: bigint): bigint {
  if (supplyBase === 0n) throw new Error("supply is zero: no NAV");
  if (treasuryMicros < 0n || supplyBase < 0n) throw new Error("negative input");
  return (treasuryMicros * PICO) / supplyBase;
}

export function feeAccrualPerPeriod(navGross: bigint, feeBps: bigint, deltaSeconds: bigint): bigint {
  if (feeBps < 0n || deltaSeconds < 0n) throw new Error("negative input");
  return (navGross * feeBps * deltaSeconds) / (10_000n * SECONDS_PER_YEAR);
}

export function weeklyPublish(treasuryMicros: bigint, supplyBase: bigint, feeBps: bigint, deltaSeconds: bigint) {
  const gross = navGrossPico(treasuryMicros, supplyBase);
  const fee = feeAccrualPerPeriod(gross, feeBps, deltaSeconds);
  if (fee > gross) throw new Error("fee exceeds NAV");
  return { gross, fee, published: gross - fee };
}

export interface NavInputs {
  model_version: number;
  asset_mint: string;
  supply_base: string;
  /** Where the treasury value was read: `sleeve:<usdc token account>` or `treasury:<npcs-treasury PDA>`. */
  treasury_source: string;
  treasury_account: string;
  treasury_micros: string;
  slot: string;
  fee_bps: string;
  delta_seconds: string;
  previous_computed_ts: string;
  computed_ts: string;
}

/** Keys sorted, no whitespace: the bytes the hash commits to. */
export function canonical(inputs: NavInputs): string {
  const sorted = Object.fromEntries(Object.entries(inputs).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
  return JSON.stringify(sorted);
}

export function inputsHash(inputs: NavInputs): Uint8Array {
  return new Uint8Array(createHash("sha256").update(canonical(inputs)).digest());
}
