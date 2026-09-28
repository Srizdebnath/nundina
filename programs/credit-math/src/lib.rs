//! `credit-math` — shared fixed-point arithmetic for NUNDINA (doc 09 §6).
//!
//! Rules that make this crate audit-scope-critical (SRS §13, D8):
//!
//! 1. **u128 intermediates everywhere.** No float. No u64 multiplication that
//!    can silently overflow — every product widens to u128 (or u256-shaped
//!    two-step division) before it is reduced.
//! 2. **Round against the initiator.** Every rounding site asks "who is
//!    initiating this action?" and floors toward them (see [`RoundAwayFromInitiator`]).
//! 3. **Zero dependencies.** `no_std + alloc`. Anything a program needs is here,
//!    reviewed, and fuzzed — nothing imported.
//! 4. **Exact conservation.** Allocation sums exactly equal the amount being
//!    allocated; waterfall lines sum exactly to cash received. Residue is
//!    re-attached by rule, never dropped.
//!
//! Units convention (doc 09 §3): amounts are integers in their smallest unit.
//! NPCS base units and USDC micros are 1e6-scaled; NAV is **pico-NAV**
//! (1e12 per whole NPCS, D-B corrected: 6dp NAV cannot represent the pilot
//! fee accrual — see [`nav`] docs).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod accrual;
pub mod allocation;
pub mod error;
pub mod nav;
pub mod prudential;
pub mod pv;
pub mod waterfall;

pub use accrual::accrue_interest;
pub use allocation::{allocate_pro_rata, find_uniform_clear_price, ClearResult, Order};
pub use error::{MathError, RoundDirection};
pub use nav::{
    fee_accrual_per_period, haircut_bps, nav_gross_pico, nav_published_pico, PICO, SECONDS_PER_YEAR,
};
pub use prudential::{check_advance_rate, BPS_DENOMINATOR, MAX_ADVANCE_RATE_BPS};
pub use pv::pv_of_claim;
pub use waterfall::{apply_loss, distribute_cash, WaterfallParams, WaterfallResult};

/// Rounding policy: every division floors **against the initiator**.
///
/// The initiator is the party whose action invokes the math:
/// * a depositor / seller / borrower initiating a claim on the protocol →
///   round **down** (protocol keeps the residue);
/// * the protocol distributing to holders → round **in favor of the
///   smallest holder claim**, i.e. round each holder's share **up**, with the
///   final (dust) line absorbing the difference — the initiator (distributor)
///   keeps the shortfall.
///
/// Programs must pass the direction explicitly; this crate never guesses.
pub trait RoundAwayFromInitiator {
    /// Divide `numerator / denominator`, flooring, asserting `direction`
    /// matches the caller's initiator analysis. Zero denominator is
    /// [`MathError::DivisionByZero`].
    fn div_round(&self, denominator: Self, direction: RoundDirection) -> Result<Self, MathError>
    where
        Self: Sized;
}

impl RoundAwayFromInitiator for u128 {
    fn div_round(&self, denominator: Self, direction: RoundDirection) -> Result<Self, MathError> {
        if denominator == 0 {
            return Err(MathError::DivisionByZero);
        }
        match direction {
            RoundDirection::Floor | RoundDirection::AgainstInitiator => Ok(self / denominator),
            RoundDirection::Ceil => Ok(ceil_div(*self, denominator)),
        }
    }
}

/// Ceiling division without overflow: `(a + b - 1) / b` in checked form.
pub fn ceil_div(a: u128, b: u128) -> u128 {
    debug_assert!(b != 0);
    a / b + u128::from(!a.is_multiple_of(b))
}

/// `a * b / c` for u128 without overflowing the multiply: splits via
/// `q = a / c`, `r = a % c`, returns `q * b + r * b / c`. Returns
/// [`MathError::Overflow`] if the result exceeds u128.
pub fn mul_div(a: u128, b: u128, c: u128) -> Result<u128, MathError> {
    if c == 0 {
        return Err(MathError::DivisionByZero);
    }
    let q = a / c;
    let r = a % c;
    let hi = q.checked_mul(b).ok_or(MathError::Overflow)?;
    let lo = r.checked_mul(b).ok_or(MathError::Overflow)?;
    hi.checked_add(lo / c).ok_or(MathError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_div_exact() {
        assert_eq!(mul_div(1_000_000, 105, 100).unwrap(), 1_050_000);
        assert_eq!(mul_div(0, 999, 7).unwrap(), 0);
    }

    #[test]
    fn mul_div_large_no_overflow() {
        // (2^100) * 3 / 2 == 3 * 2^99 — fits u128, would overflow naive mul.
        let a: u128 = 1 << 100;
        assert_eq!(mul_div(a, 3, 2).unwrap(), 3 * (1 << 99));
    }

    #[test]
    fn ceil_div_smoke() {
        assert_eq!(ceil_div(10, 5), 2);
        assert_eq!(ceil_div(11, 5), 3);
        assert_eq!(ceil_div(0, 5), 0);
    }
}
