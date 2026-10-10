//! QC-P3 PV model: `E[fill_t]` from the gate percentage, the queue depth
//! and the fill history (doc 06 §5, "PV model: E[fill_t] from gate pct,
//! queue depth, fill history"), the deterministic part on-chain (#202).
//!
//! The issuer / transfer-agent bridge publishes the inputs into the
//! queue's [`QueueConfig`](crate::QueueConfig) each window
//! (`set_fill_model`); `quote_pv_model` turns them into an expected-fill
//! schedule and prices it with `credit_math::pv_of_claim`. The attested
//! per-request schedule (`quote_pv`) stays as the fallback when no model
//! is published.
//!
//! Per window the gate redeems `fund_supply × gate_bps` units against a
//! queue of `queue_depth` units, so a pro-rated request fills
//! `ratio = min(1, fund_supply × gate_bps / queue_depth)` of what it still
//! has outstanding; a published fill history caps the ratio (the lower of
//! model and experience, the conservative side). The queue depth is held
//! constant across the horizon (new requests replace filled ones). Each
//! expected fill is floored; whatever is still outstanding after the
//! horizon is valued at zero.

use anchor_lang::prelude::*;

/// Hard ceiling on the model horizon (windows).
pub const MAX_MODEL_PERIODS: u32 = 16;
const BPS: u64 = 10_000;

#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace, Default,
)]
pub struct FillModel {
    /// Share of the fund redeemable per window (bps of `fund_supply`).
    pub gate_bps: u64,
    /// Units outstanding in the fund (base units).
    pub fund_supply: u64,
    /// Units requested across the whole queue, this request included.
    pub queue_depth: u64,
    /// Historical fill ratio per window (bps; 0 = no history yet).
    pub fill_history_bps: u64,
    /// Windows valued (1..=`MAX_MODEL_PERIODS`).
    pub periods: u32,
}

impl FillModel {
    pub fn is_valid(&self) -> bool {
        self.gate_bps <= BPS
            && self.fill_history_bps <= BPS
            && self.fund_supply > 0
            && (1..=MAX_MODEL_PERIODS).contains(&self.periods)
    }

    /// Per-window fill ratio, bps of the outstanding quantity.
    pub fn ratio_bps(&self) -> u64 {
        let pro_rata = if self.queue_depth == 0 {
            BPS
        } else {
            let r = u128::from(self.fund_supply) * u128::from(self.gate_bps)
                / u128::from(self.queue_depth);
            u64::try_from(r).unwrap_or(u64::MAX).min(BPS)
        };
        if self.fill_history_bps > 0 {
            pro_rata.min(self.fill_history_bps)
        } else {
            pro_rata
        }
    }

    /// `(period, qty)` expected fills of `outstanding` units.
    pub fn expected_fills(&self, outstanding: u64) -> Vec<(u32, u64)> {
        let ratio = u128::from(self.ratio_bps());
        let mut left = outstanding;
        let mut out = Vec::new();
        for period in 0..self.periods.min(MAX_MODEL_PERIODS) {
            let fill = u64::try_from(u128::from(left) * ratio / u128::from(BPS)).unwrap_or(0);
            if fill == 0 {
                break;
            }
            out.push((period, fill));
            left -= fill;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(gate_bps: u64, fund: u64, depth: u64, hist: u64, periods: u32) -> FillModel {
        FillModel {
            gate_bps,
            fund_supply: fund,
            queue_depth: depth,
            fill_history_bps: hist,
            periods,
        }
    }

    #[test]
    fn under_subscribed_gate_fills_everything_at_once() {
        // 5% of 1_000 = 50 per window against a queue of 40: all at t=0.
        let f = m(500, 1_000, 40, 0, 4);
        assert_eq!(f.ratio_bps(), BPS);
        assert_eq!(f.expected_fills(40), vec![(0, 40)]);
    }

    #[test]
    fn over_subscribed_gate_pro_rates_geometrically() {
        // 5% of 1_000 = 50 against a 200 queue: 25% of what is left.
        let f = m(500, 1_000, 200, 0, 3);
        assert_eq!(f.ratio_bps(), 2_500);
        assert_eq!(f.expected_fills(100), vec![(0, 25), (1, 18), (2, 14)]);
    }

    #[test]
    fn history_caps_the_model() {
        let f = m(500, 1_000, 200, 1_000, 2);
        assert_eq!(f.ratio_bps(), 1_000);
        assert_eq!(f.expected_fills(100), vec![(0, 10), (1, 9)]);
        // History above the model does not raise it.
        assert_eq!(m(500, 1_000, 200, 9_000, 2).ratio_bps(), 2_500);
    }

    #[test]
    fn floors_stop_the_schedule_and_bounds_are_checked() {
        assert_eq!(m(1, 1, 10_000, 0, 8).expected_fills(5), vec![]);
        assert!(m(500, 1_000, 200, 0, 3).is_valid());
        assert!(!m(10_001, 1_000, 200, 0, 3).is_valid());
        assert!(!m(500, 0, 200, 0, 3).is_valid());
        assert!(!m(500, 1_000, 200, 0, 0).is_valid());
        assert!(!m(500, 1_000, 200, 0, MAX_MODEL_PERIODS + 1).is_valid());
        assert!(!m(500, 1_000, 200, 10_001, 3).is_valid());
    }
}
