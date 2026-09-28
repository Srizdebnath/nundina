//! NAV arithmetic — invariants I1/I2 (doc 09 §3) and §6 function inventory.
//!
//! **Precision decision (D-B correction):** NAV is carried at **pico**
//! precision — [`PICO`] = 10¹² sub-units per whole NPCS. At $3K / 10bps the
//! weekly fee on NAV 1.0 is 19_178_082 pico, i.e. **≈ 19.18 µUSDC of NAV per
//! NPCS**. A 6dp (micro-USDC) NAV can hold that, but truncates up to 1 µUSDC
//! (~5% of the fee) at every publish. At pico precision the fee is exact.
//!
//! All functions floor (`div_round(…, RoundDirection::AgainstInitiator)`): the publisher
//! (NUNDINA, in the pilot) is the initiator of a mark, so rounding goes
//! against the mark — never inflating NAV.

use crate::error::{MathError, RoundDirection};
use crate::prudential::BPS_DENOMINATOR;
use crate::{mul_div, RoundAwayFromInitiator};

/// Whole NAV per NPCS in pico (1e12). NAV of 1.000411 USDC/NPCS = 1_000_411_522_333 pico.
pub const PICO: u128 = 1_000_000_000_000;

/// Julian year in seconds (31,536,000) — the fee-accrual clock.
pub const SECONDS_PER_YEAR: u64 = 31_536_000;

/// I2a — gross NAV from real treasury value.
///
/// * `treasury_usdc_micros` — sleeve value in USDC micros (1e-6).
/// * `supply_base` — NPCS base units outstanding (1e-6 scale, I1).
///
/// Returns NAV in pico-USDC per whole NPCS:
/// `floor(treasury_micros × 1e12 / supply_base)`.
pub fn nav_gross_pico(treasury_usdc_micros: u128, supply_base: u128) -> Result<u128, MathError> {
    if supply_base == 0 {
        return Err(MathError::DivisionByZero);
    }
    // treasury_micros (≤ ~1e12 at pilot) × 1e12 ≤ ~1e24 — no overflow at any
    // realistic scale; checked anyway.
    let numer = treasury_usdc_micros
        .checked_mul(PICO)
        .ok_or(MathError::Overflow)?;
    // The publisher initiates the mark: floor against it, never inflate NAV.
    numer.div_round(supply_base, RoundDirection::AgainstInitiator)
}

/// I2b — fee accrual for one publication period, in pico.
///
/// `floor(nav_pico × fee_bps × Δt_seconds / (10⁴ × 31_536_000))`
///
/// Example (exact): nav = 1.0 NPCS (1e12 pico), fee_bps = 10, Δt = 7 days →
/// **19_178_082 pico**. The publisher is the initiator → floor.
pub fn fee_accrual_per_period(
    nav_gross_pico: u128,
    fee_bps: u64,
    delta_seconds: u64,
) -> Result<u128, MathError> {
    let denom = 10_000u128
        .checked_mul(u128::from(SECONDS_PER_YEAR))
        .ok_or(MathError::Overflow)?;
    let numer = nav_gross_pico
        .checked_mul(u128::from(fee_bps))
        .and_then(|x| x.checked_mul(u128::from(delta_seconds)))
        .ok_or(MathError::Overflow)?;
    numer.div_round(denom, RoundDirection::AgainstInitiator)
}

/// I2c — published NAV: gross minus accrued fee (publisher floors against itself).
pub fn nav_published_pico(nav_gross_pico: u128, fee_pico: u128) -> Result<u128, MathError> {
    nav_gross_pico
        .checked_sub(fee_pico)
        .ok_or(MathError::Underflow)
}

/// Haircut application (ME-P3): `nav_effective = nav × (10⁴ − h) / 10⁴`,
/// with `h ≤ ceiling_bps ≤ 10⁴` enforced. Floors against the mark consumer.
///
/// `ceiling_bps` is supplied by the caller. It is rejected above 100%
/// (a haircut cannot exceed the whole NAV). The doc 06 §11 H3 ceiling has
/// no plan-of-record value yet, so it is not a crate constant (see
/// [`crate::prudential`]).
pub fn haircut_bps(nav_pico: u128, haircut: u64, ceiling_bps: u64) -> Result<u128, MathError> {
    if ceiling_bps > BPS_DENOMINATOR || haircut > ceiling_bps {
        return Err(MathError::InvalidInput);
    }
    mul_div(
        nav_pico,
        u128::from(BPS_DENOMINATOR - haircut),
        u128::from(BPS_DENOMINATOR),
    )
}

/// Convenience: the full weekly publish chain in one call.
/// Returns `(nav_gross, fee, nav_published)`, all pico.
pub fn weekly_publish(
    treasury_usdc_micros: u128,
    supply_base: u128,
    fee_bps: u64,
    delta_seconds: u64,
) -> Result<(u128, u128, u128), MathError> {
    let g = nav_gross_pico(treasury_usdc_micros, supply_base)?;
    let f = fee_accrual_per_period(g, fee_bps, delta_seconds)?;
    let p = nav_published_pico(g, f)?;
    Ok((g, f, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_gross_hand_vector_pilot() {
        // $3,001.234567 sleeve / 3,000 NPCS:
        // (3_001_234_567 × 1e12) / 3_000_000_000 = 1_000_411_522_333.33… → floor
        assert_eq!(
            nav_gross_pico(3_001_234_567, 3_000_000_000).unwrap(),
            1_000_411_522_333
        );
    }

    #[test]
    fn nav_gross_rejects_zero_supply() {
        assert_eq!(
            nav_gross_pico(1_000_000, 0).unwrap_err(),
            MathError::DivisionByZero
        );
    }

    #[test]
    fn fee_exact_at_unit_nav() {
        // 1e12 pico × 10 bps × 604_800 s / (10⁴ × 31_536_000) = 19_178_082.19… → floor
        assert_eq!(
            fee_accrual_per_period(1_000_000_000_000, 10, 604_800).unwrap(),
            19_178_082
        );
    }

    #[test]
    fn fee_zero_when_no_time_passes() {
        assert_eq!(fee_accrual_per_period(1_000_000_000_000, 10, 0).unwrap(), 0);
    }

    #[test]
    fn published_never_exceeds_gross() {
        let g = 1_000_411_522_333;
        let f = fee_accrual_per_period(g, 10, 604_800).unwrap();
        assert!(nav_published_pico(g, f).unwrap() < g);
    }

    #[test]
    fn haircut_respects_ceiling() {
        let nav = 1_000_000_000_000;
        assert_eq!(haircut_bps(nav, 0, 2_000).unwrap(), nav);
        assert_eq!(haircut_bps(nav, 200, 2_000).unwrap(), 980_000_000_000);
        assert_eq!(
            haircut_bps(nav, 2_001, 2_000).unwrap_err(),
            MathError::InvalidInput
        );
    }

    #[test]
    fn haircut_ceiling_bounds() {
        let nav = 1_000_000_000_000;
        // ceiling 0: only a zero haircut passes.
        assert_eq!(haircut_bps(nav, 0, 0).unwrap(), nav);
        assert_eq!(haircut_bps(nav, 1, 0).unwrap_err(), MathError::InvalidInput);
        // ceiling 100%: a full haircut is representable.
        assert_eq!(haircut_bps(nav, 10_000, 10_000).unwrap(), 0);
        // ceiling above 100% is rejected before it can underflow `10⁴ − h`.
        assert_eq!(
            haircut_bps(nav, 15_000, 20_000).unwrap_err(),
            MathError::InvalidInput
        );
        assert_eq!(
            haircut_bps(nav, 0, 20_000).unwrap_err(),
            MathError::InvalidInput
        );
        assert_eq!(
            haircut_bps(nav, u64::MAX, u64::MAX).unwrap_err(),
            MathError::InvalidInput
        );
    }

    #[test]
    fn weekly_publish_chain() {
        let (g, f, p) = weekly_publish(3_001_234_567, 3_000_000_000, 10, 604_800).unwrap();
        assert_eq!(g, 1_000_411_522_333);
        assert_eq!(p, g - f);
    }
}
