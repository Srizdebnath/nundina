//! Waterfall arithmetic — top-down cash distribution and bottom-up loss
//! application (doc 09 §6; `06` §6; TE-P3/P4).
//!
//! Conservation laws (property-tested):
//! * **Cash:** every paid line + junior residual == cash received, exactly.
//!   The residual line is the absorber by rule; nothing is dropped.
//! * **Loss:** junior principal absorbs first, then reserve, then senior.
//!   A loss exceeding the total stack is an input error, not a silent wipe.
//!
//! PIK is structurally excluded here: this engine distributes *cash* only
//! (B8); PIK accrual lives on the mark book, never in the waterfall input.

use crate::error::MathError;

/// Priority-ordered obligations for one distribution period (USDC micros).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaterfallParams {
    /// 1 · fees & expenses (manager/LPA line).
    pub fees_due: u128,
    /// 2 · senior accrued coupon, cash-funded.
    pub senior_coupon_due: u128,
    /// 3 · reserve top-up target: reserve is refilled to this level.
    pub reserve_target: u128,
    /// Current reserve balance before this period.
    pub reserve_current: u128,
    /// 4 · senior amortization due (Amortizing state).
    pub senior_amort_due: u128,
    /// 5 · junior coupon due.
    pub junior_coupon_due: u128,
}

/// Outcome of one cash distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaterfallResult {
    /// Line 1 actually paid.
    pub paid_fees: u128,
    /// Line 2 actually paid.
    pub paid_senior_coupon: u128,
    /// Line 3 actually paid (deficit refill).
    pub paid_reserve: u128,
    /// Line 4 actually paid.
    pub paid_senior_amort: u128,
    /// Line 5 actually paid.
    pub paid_junior_coupon: u128,
    /// Line 6 — all remaining cash after the priority lines.
    pub junior_residual: u128,
    /// Senior coupon shortfall — defers (carried), never a default event.
    pub unpaid_senior_coupon: u128,
    /// Senior amortization shortfall — defers, never a default event.
    pub unpaid_senior_amort: u128,
    /// Junior coupon shortfall — defers.
    pub unpaid_junior_coupon: u128,
}

impl WaterfallResult {
    /// Conservation check: paid lines + residual == the cash distributed.
    pub fn total_distributed(&self) -> Result<u128, MathError> {
        self.paid_fees
            .checked_add(self.paid_senior_coupon)
            .and_then(|x| x.checked_add(self.paid_reserve))
            .and_then(|x| x.checked_add(self.paid_senior_amort))
            .and_then(|x| x.checked_add(self.paid_junior_coupon))
            .and_then(|x| x.checked_add(self.junior_residual))
            .ok_or(MathError::Overflow)
    }
}

/// Distribute `cash_micros` down the priority stack (TE-P4 order).
/// Deterministic; every line is `min(due, remaining)`; the junior residual
/// absorbs whatever remains.
pub fn distribute_cash(
    cash_micros: u128,
    p: &WaterfallParams,
) -> Result<WaterfallResult, MathError> {
    let mut remaining = cash_micros;

    // 1 · fees
    let paid_fees = remaining.min(p.fees_due);
    remaining -= paid_fees;

    // 2 · senior coupon (shortfall defers)
    let paid_senior_coupon = remaining.min(p.senior_coupon_due);
    remaining -= paid_senior_coupon;
    let unpaid_senior_coupon = p.senior_coupon_due - paid_senior_coupon;

    // 3 · reserve top-up: refill toward target, bounded by cash
    let reserve_deficit = p.reserve_target.saturating_sub(p.reserve_current);
    let paid_reserve = remaining.min(reserve_deficit);
    remaining -= paid_reserve;

    // 4 · senior amortization (shortfall defers)
    let paid_senior_amort = remaining.min(p.senior_amort_due);
    remaining -= paid_senior_amort;
    let unpaid_senior_amort = p.senior_amort_due - paid_senior_amort;

    // 5 · junior coupon (shortfall defers)
    let paid_junior_coupon = remaining.min(p.junior_coupon_due);
    remaining -= paid_junior_coupon;
    let unpaid_junior_coupon = p.junior_coupon_due - paid_junior_coupon;

    // 6 · junior residual — the absorber
    let junior_residual = remaining;

    Ok(WaterfallResult {
        paid_fees,
        paid_senior_coupon,
        paid_reserve,
        paid_senior_amort,
        paid_junior_coupon,
        junior_residual,
        unpaid_senior_coupon,
        unpaid_senior_amort,
        unpaid_junior_coupon,
    })
}

/// Outcome of a realized loss (bottom-up, E2/E12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossResult {
    /// Realized-loss book: hits junior principal, cash terms.
    pub junior_hit: u128,
    /// Reserve consumed after junior is exhausted.
    pub reserve_hit: u128,
    /// Senior principal impaired after junior + reserve are exhausted.
    pub senior_hit: u128,
    /// Reserve balance after the loss.
    pub reserve_after: u128,
}

/// Apply a realized loss bottom-up: junior principal → reserve → senior.
/// A loss exceeding junior + reserve + senior is `InvalidInput` (structurally
/// impossible while the stack is fully funded — E12 demands explicitness,
/// never a wrap).
pub fn apply_loss(
    loss_micros: u128,
    junior_principal_micros: u128,
    reserve_current_micros: u128,
    senior_principal_micros: u128,
) -> Result<LossResult, MathError> {
    let total_absorb = junior_principal_micros
        .checked_add(reserve_current_micros)
        .and_then(|x| x.checked_add(senior_principal_micros))
        .ok_or(MathError::Overflow)?;
    if loss_micros > total_absorb {
        return Err(MathError::InvalidInput);
    }

    let junior_hit = loss_micros.min(junior_principal_micros);
    let mut rest = loss_micros - junior_hit;

    let reserve_hit = rest.min(reserve_current_micros);
    rest -= reserve_hit;
    let reserve_after = reserve_current_micros - reserve_hit;

    let senior_hit = rest; // ≤ senior_principal by the total_absorb check

    Ok(LossResult {
        junior_hit,
        reserve_hit,
        senior_hit,
        reserve_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> WaterfallParams {
        WaterfallParams {
            fees_due: 100,
            senior_coupon_due: 500,
            reserve_target: 1_000,
            reserve_current: 800,
            senior_amort_due: 400,
            junior_coupon_due: 300,
        }
    }

    #[test]
    fn full_cascade_everything_paid() {
        // Needs: 100 + 500 + 200(top-up) + 400 + 300 = 1_500; send 2_000
        // → junior residual 500.
        let r = distribute_cash(2_000, &params()).unwrap();
        assert_eq!(r.paid_fees, 100);
        assert_eq!(r.paid_senior_coupon, 500);
        assert_eq!(r.paid_reserve, 200);
        assert_eq!(r.paid_senior_amort, 400);
        assert_eq!(r.paid_junior_coupon, 300);
        assert_eq!(r.junior_residual, 500);
        assert_eq!(r.total_distributed().unwrap(), 2_000);
        assert_eq!(r.unpaid_senior_coupon, 0);
    }

    #[test]
    fn shortfall_stops_at_cash() {
        // 600 cash: fees 100, senior coupon 500 (fully paid), reserve 0 →
        // unpaid amort 400, unpaid junior 300, residual 0.
        let r = distribute_cash(600, &params()).unwrap();
        assert_eq!(r.paid_fees, 100);
        assert_eq!(r.paid_senior_coupon, 500);
        assert_eq!(r.paid_reserve, 0);
        assert_eq!(r.unpaid_senior_amort, 400);
        assert_eq!(r.unpaid_junior_coupon, 300);
        assert_eq!(r.junior_residual, 0);
        assert_eq!(r.total_distributed().unwrap(), 600);
    }

    #[test]
    fn senior_coupon_defers_before_reserve() {
        // 300 cash: fees 100, coupon gets 200 of 500 → unpaid 300,
        // reserve untouched (paid 0).
        let r = distribute_cash(300, &params()).unwrap();
        assert_eq!(r.paid_senior_coupon, 200);
        assert_eq!(r.unpaid_senior_coupon, 300);
        assert_eq!(r.paid_reserve, 0);
        assert_eq!(r.total_distributed().unwrap(), 300);
    }

    #[test]
    fn loss_absorbed_by_junior_only() {
        let r = apply_loss(300, 1_000, 500, 3_000).unwrap();
        assert_eq!(r.junior_hit, 300);
        assert_eq!(r.reserve_hit, 0);
        assert_eq!(r.senior_hit, 0);
        assert_eq!(r.reserve_after, 500);
    }

    #[test]
    fn loss_spills_to_reserve_then_senior() {
        // 1_400 loss: junior 1_000, reserve 400, senior 0; reserve left 100.
        let r = apply_loss(1_400, 1_000, 500, 3_000).unwrap();
        assert_eq!(r.junior_hit, 1_000);
        assert_eq!(r.reserve_hit, 400);
        assert_eq!(r.reserve_after, 100);
        assert_eq!(r.senior_hit, 0);
    }

    #[test]
    fn loss_impairs_senior() {
        // 4_400 of 4_500 stack: junior 1_000, reserve 500, senior 2_900.
        let r = apply_loss(4_400, 1_000, 500, 3_000).unwrap();
        assert_eq!(r.junior_hit, 1_000);
        assert_eq!(r.reserve_hit, 500);
        assert_eq!(r.senior_hit, 2_900);
        assert_eq!(r.reserve_after, 0);
    }

    #[test]
    fn loss_beyond_stack_is_invalid() {
        // 4_501 > 1_000 + 500 + 3_000 = 4_500.
        assert_eq!(
            apply_loss(4_501, 1_000, 500, 3_000).unwrap_err(),
            MathError::InvalidInput
        );
    }

    #[test]
    fn zero_loss_is_noop() {
        let r = apply_loss(0, 1_000, 500, 3_000).unwrap();
        assert_eq!(
            (r.junior_hit, r.reserve_hit, r.senior_hit, r.reserve_after),
            (0, 0, 0, 500)
        );
    }
}
