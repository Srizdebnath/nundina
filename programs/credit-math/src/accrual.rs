//! Interest accrual for liquidity-facility advances (doc 09 §6; LF-P6).
//!
//! `floor(principal × rate_bps × Δt_seconds / (10⁴ × 31_536_000))`, with the
//! same Julian-year clock as the NAV fee. Pilot uses a fixed 12% APR (LF-P6).
//!
//! **Rounding: `Floor`, at every call.** There is no separate settle-time
//! round-up. The sub-micro remainder of each accrual (< 1 µUSDC per call)
//! stays with the borrower. Strictly, rule 2 in the crate docs would round
//! *up* here, because the borrower initiates `repay`. The floor is a
//! deliberate, bounded exception: `payment-rails`' live facility accrues with
//! it, and changing it is a rails upgrade.

use crate::error::{MathError, RoundDirection};
use crate::nav::SECONDS_PER_YEAR;
use crate::RoundAwayFromInitiator;

/// Accrued interest over `delta_seconds` on `principal_micros` at
/// `rate_bps` annual, floored. Pure function — no state.
pub fn accrue_interest(
    principal_micros: u128,
    rate_bps: u64,
    delta_seconds: u64,
) -> Result<u128, MathError> {
    let denom = 10_000u128
        .checked_mul(u128::from(SECONDS_PER_YEAR))
        .ok_or(MathError::Overflow)?;
    let numer = principal_micros
        .checked_mul(u128::from(rate_bps))
        .and_then(|x| x.checked_mul(u128::from(delta_seconds)))
        .ok_or(MathError::Overflow)?;
    numer.div_round(denom, RoundDirection::Floor)
}

/// Balance after accrual: principal + floored interest, overflow-checked.
pub fn balance_after(
    principal_micros: u128,
    rate_bps: u64,
    delta_seconds: u64,
) -> Result<u128, MathError> {
    let i = accrue_interest(principal_micros, rate_bps, delta_seconds)?;
    principal_micros.checked_add(i).ok_or(MathError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_percent_annual_on_thousand() {
        // 1_000_000_000 μUSD × 1200 bps × 31_536_000 s / (10⁴ × 31_536_000)
        // = 1_000_000_000 × 0.12 = 120_000_000 exactly.
        assert_eq!(
            accrue_interest(1_000_000_000, 1_200, 31_536_000).unwrap(),
            120_000_000
        );
    }

    #[test]
    fn per_day_is_floored() {
        // 1_000_000_000 μUSD × 1200 bps × 86_400/31_536_000 = 328_767.12… → 328_767
        // ($1,000 at 12% APR = $0.3288/day.)
        assert_eq!(
            accrue_interest(1_000_000_000, 1_200, 86_400).unwrap(),
            328_767
        );
    }

    #[test]
    fn zero_rate_zero_time() {
        assert_eq!(accrue_interest(1_000_000_000, 0, 86_400).unwrap(), 0);
        assert_eq!(accrue_interest(1_000_000_000, 1_200, 0).unwrap(), 0);
    }

    #[test]
    fn balance_composes() {
        assert_eq!(
            balance_after(1_000_000_000, 1_200, 31_536_000).unwrap(),
            1_120_000_000
        );
    }
}
