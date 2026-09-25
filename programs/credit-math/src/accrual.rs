//! Interest accrual for liquidity-facility advances (doc 09 §6; LF-P6).
//!
//! `floor(principal × rate_bps × Δt_seconds / (10⁴ × 31_536_000))`
//! — same clock as the NAV fee (Julian year), same floor-against-initiator
//! rule: the *borrower* initiates time passing on their own debt position
//! only at repayment; accrual is computed by the protocol at every touch,
//! and floors **in the borrower's favor is forbidden** — the lender-side
//! initiator rule means accrued interest rounds **up** to the next micro
//! only at explicit `settle` boundaries, and is carried as a floored
//! running value between them. Pilot uses fixed 12% APR (LF-P6).

use crate::error::MathError;
use crate::nav::SECONDS_PER_YEAR;

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
    Ok(numer / denom)
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
