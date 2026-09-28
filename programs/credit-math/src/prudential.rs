//! Prudential floor — doc 06 §11 `HARD` ("outside any governance reach", L6).
//!
//! A bound lives here, as a crate constant, only when the plan of record
//! fixes its value. Programs check curator parameters against these
//! constants; nothing in governance can raise them short of a code change
//! (and therefore a program upgrade).
//!
//! | Item | Plan of record | Status in this crate |
//! |---|---|---|
//! | H1 max advance rate | 50% (SRS 08 LF-P1, doc 07 LF-3) | [`MAX_ADVANCE_RATE_BPS`] + [`check_advance_rate`]. No program has an advance-rate field yet |
//! | H2 min subordination floor | no value set | **not enforced**: no tranche program exists |
//! | H3 max discount / reserve-price bound | no value set | **not enforced** beyond the structural `≤ 100%` in [`crate::nav::haircut_bps`] |
//! | H4 no liquidation on a stale mark | rule, not a number | **not enforced**: no mark engine, no liquidation instruction |
//! | H5 timeout refund path | rule, not a number | enforced by `payment-rails` refunds (see its PHASES.md for the ask-vault caveat) |
//!
//! Do not add a constant for H2/H3 with a guessed value. The number has to
//! come from the plan first.
//!
//! These are **not** the `payment-rails` caps (`MAX_FEE_BPS`,
//! `MAX_RESERVE_BPS`, `MAX_RATE_BPS`). Those bound the rails' fee, reserve
//! skim and facility *interest* APR, which are different quantities.

use crate::error::MathError;

/// Basis-point denominator: 10 000 bps = 100%.
pub const BPS_DENOMINATOR: u64 = 10_000;

/// H1: hard ceiling on a facility advance rate (loan / collateral mark), in
/// bps. 5 000 = 50% (SRS 08 LF-P1: "advance_rate ≤ hard on-chain ceiling (50%)").
pub const MAX_ADVANCE_RATE_BPS: u64 = 5_000;

/// Rejects an advance rate above [`MAX_ADVANCE_RATE_BPS`] with
/// [`MathError::InvalidInput`]. Curator parameters sit below it.
pub fn check_advance_rate(advance_rate_bps: u64) -> Result<(), MathError> {
    if advance_rate_bps > MAX_ADVANCE_RATE_BPS {
        return Err(MathError::InvalidInput);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_rate_ceiling_is_fifty_percent() {
        assert_eq!(MAX_ADVANCE_RATE_BPS * 2, BPS_DENOMINATOR);
        assert!(check_advance_rate(0).is_ok());
        assert!(check_advance_rate(5_000).is_ok());
        assert_eq!(
            check_advance_rate(5_001).unwrap_err(),
            MathError::InvalidInput
        );
        assert_eq!(
            check_advance_rate(u64::MAX).unwrap_err(),
            MathError::InvalidInput
        );
    }
}
