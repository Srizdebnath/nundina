//! Present-value arithmetic for queue-claims (doc 09 §6; QC-P3).
//!
//! A redemption-queue claim fills over future gated windows. Its model
//! value is the discounted sum of expected fills:
//!
//! `PV = Σ_t floor( qty_t × price_t / 10⁶ × dᵗ )`, `d = (10⁴ − bps)/10⁴`
//! applied per period (period 0 = next window, undiscounted).
//!
//! Floors at every step (the *seller* of a claim initiates the valuation →
//! value rounds against them → conservative — what the facility underwrites
//! with, MARK_PROTECTIVE style).

use crate::error::MathError;

/// One expected fill in the schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedFill {
    /// Period index (0 = next window), used as the discount exponent.
    pub period: u32,
    /// Expected fill in NPCS base units.
    pub qty_base: u128,
    /// Expected execution price in USDC micros per NPCS (pilot: NAV).
    pub price_micros: u128,
}

/// `pv_of_claim`: discounted value of a fill schedule, in USDC micros.
///
/// Each term is computed as `qty × price / 10⁶` (base units → micro-USDC),
/// then multiplied down by `d` once per period with a floor each step.
pub fn pv_of_claim(
    fills: &[ExpectedFill],
    discount_bps_per_period: u64,
) -> Result<u128, MathError> {
    if discount_bps_per_period > 10_000 {
        return Err(MathError::InvalidInput);
    }
    let keep = 10_000u128 - u128::from(discount_bps_per_period);
    let mut total: u128 = 0;
    for f in fills {
        // qty_base (1e-6 NPCS) × price_micros (1e-6 USDC) / 1e6 = micro-USDC.
        let term0 = f
            .qty_base
            .checked_mul(f.price_micros)
            .ok_or(MathError::Overflow)?
            / 1_000_000;
        let mut term = term0;
        for _ in 0..f.period {
            term = term.checked_mul(keep).ok_or(MathError::Overflow)? / 10_000;
        }
        total = total.checked_add(term).ok_or(MathError::Overflow)?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undiscounted_single_fill() {
        // 1_000 NPCS × 1.00 USDC → 1_000_000_000 micros (0 bps discount).
        let fills = [ExpectedFill {
            period: 0,
            qty_base: 1_000_000_000,
            price_micros: 1_000_000,
        }];
        assert_eq!(pv_of_claim(&fills, 0).unwrap(), 1_000_000_000);
    }

    #[test]
    fn discount_applies_geometrically() {
        // 1_000 NPCS @ 1.00, 100bps/period, periods 0 and 1:
        // t0: 1_000_000_000; t1: ×0.99 → 990_000_000
        let fills = [
            ExpectedFill {
                period: 0,
                qty_base: 1_000_000_000,
                price_micros: 1_000_000,
            },
            ExpectedFill {
                period: 1,
                qty_base: 1_000_000_000,
                price_micros: 1_000_000,
            },
        ];
        assert_eq!(pv_of_claim(&fills, 100).unwrap(), 1_990_000_000);
    }

    #[test]
    fn three_periods_floor_chain() {
        // t=3, 100bps: 1_000_000_000 → 990_000_000 → 980_100_000 → 970_299_000
        let fills = [ExpectedFill {
            period: 3,
            qty_base: 1_000_000_000,
            price_micros: 1_000_000,
        }];
        assert_eq!(pv_of_claim(&fills, 100).unwrap(), 970_299_000);
    }

    #[test]
    fn rejects_over_total_discount() {
        let fills = [ExpectedFill {
            period: 0,
            qty_base: 1,
            price_micros: 1,
        }];
        assert_eq!(
            pv_of_claim(&fills, 10_001).unwrap_err(),
            MathError::InvalidInput
        );
    }
}
