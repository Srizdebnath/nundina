//! Property-based invariants (proptest) — the SRS §13 requirement:
//! "property-based tests on clearing, pro-rata, waterfall invariants."
//!
//! Each property is an audit claim in executable form. If any of these
//! breaks under fuzz, the crate does not ship into a program.

use credit_math::accrual::accrue_interest;
use credit_math::allocation::{allocate_pro_rata, find_uniform_clear_price, Order};
use credit_math::nav::{nav_gross_pico, weekly_publish, PICO};
use credit_math::pv::{pv_of_claim, ExpectedFill};
use credit_math::waterfall::{apply_loss, distribute_cash, WaterfallParams};
use credit_math::MathError;
use proptest::prelude::*;

/// Random order book: prices around 1.0 USDC, sizes ≤ 1M base units.
fn order_strategy() -> impl Strategy<Value = Vec<Order>> {
    prop::collection::vec(
        (
            0u64..64,
            900_000u128..1_100_000,
            1u128..1_000_000,
            any::<bool>(),
        )
            .prop_map(|(bidder_id, limit_price, qty, is_buy)| Order {
                bidder_id,
                limit_price,
                qty,
                is_buy,
            }),
        0..12,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    // ---------------------------------------------------------------
    // Pro-rata: the allocation is exact. Always.
    // ---------------------------------------------------------------
    #[test]
    fn prop_pro_rata_sums_exactly(
        total in 0u128..5_000_000,
        bids in prop::collection::vec((0u64..1000, 1u128..250_000), 1..8),
    ) {
        let sum_qty: u128 = bids.iter().map(|&(_, q)| q).sum();
        let res = allocate_pro_rata(total, &bids);
        if total > sum_qty {
            // Over-allocation is a typed rejection, never a silent scale-up.
            prop_assert_eq!(res.unwrap_err(), MathError::OverAllocation);
            return Ok(());
        }
        let out = res.unwrap();
        let sum: u128 = out.iter().map(|&(_, a)| a).sum();
        prop_assert_eq!(sum, total);
        for ((id, a), (bid_id, q)) in out.iter().zip(bids.iter()) {
            prop_assert_eq!(id, bid_id);
            // Nobody is allocated more than they committed …
            prop_assert!(a <= q);
            // … nor more than their proportional ceiling (+1 dust).
            prop_assert!(*a <= q * total / sum_qty + 1);
        }
    }

    // ---------------------------------------------------------------
    // Clearing: never below reserve; executed never exceeds either side;
    // deterministic across runs.
    // ---------------------------------------------------------------
    #[test]
    fn prop_clearing_respects_reserve_and_determinism(
        orders in order_strategy(),
        reserve in 900_000u128..1_100_000,
        reference in 950_000u128..1_050_000,
    ) {
        let r1 = find_uniform_clear_price(&orders, reserve, reference).unwrap();
        let r2 = find_uniform_clear_price(&orders, reserve, reference).unwrap();
        prop_assert_eq!(&r1, &r2);
        if let Some(r) = r1 {
            prop_assert!(r.clear_price >= reserve);
            let sell: u128 = orders
                .iter()
                .filter(|o| !o.is_buy && o.limit_price <= r.clear_price)
                .map(|o| o.qty)
                .sum();
            let buy: u128 = orders
                .iter()
                .filter(|o| o.is_buy && o.limit_price >= r.clear_price)
                .map(|o| o.qty)
                .sum();
            prop_assert!(r.executed_qty > 0);
            prop_assert!(r.executed_qty <= sell);
            prop_assert!(r.executed_qty <= buy);
        }
    }

    // ---------------------------------------------------------------
    // NAV: the floor definition — gross × supply ≤ treasury × 1e12 <
    // (gross+1) × supply — and published ≤ gross, fee monotone in time.
    // ---------------------------------------------------------------
    #[test]
    fn prop_nav_floor_definition(
        treasury in 1u128..10_000_000_000_000, // ≤ $10M in micros
        supply in 1u128..10_000_000_000_000,
        fee_bps in 0u64..1_000,
        delta in 0u64..=31_536_000,
    ) {
        let g = nav_gross_pico(treasury, supply).unwrap();
        let numer = treasury.checked_mul(PICO).unwrap();
        prop_assert!(g.checked_mul(supply).unwrap() <= numer);
        prop_assert!(numer < (g + 1).checked_mul(supply).unwrap());
        let (_, f, p) = weekly_publish(treasury, supply, fee_bps, delta).unwrap();
        prop_assert!(p <= g);
        prop_assert!(f <= g);
        // Monotonicity in time: a longer period never accrues less.
        let (_, f2, _) = weekly_publish(treasury, supply, fee_bps, delta + 1).unwrap();
        prop_assert!(f2 >= f);
    }

    // ---------------------------------------------------------------
    // Waterfall cash: paid lines + residual == cash received, exactly,
    // for every cash level and every due combination.
    // ---------------------------------------------------------------
    #[test]
    fn prop_waterfall_conserves_cash(
        cash in 0u128..5_000_000,
        fees in 0u128..100_000,
        coupon in 0u128..200_000,
        res_target in 0u128..300_000,
        res_cur in 0u128..300_000,
        amort in 0u128..200_000,
        jcoupon in 0u128..100_000,
    ) {
        let p = WaterfallParams {
            fees_due: fees,
            senior_coupon_due: coupon,
            reserve_target: res_target,
            reserve_current: res_cur,
            senior_amort_due: amort,
            junior_coupon_due: jcoupon,
        };
        let r = distribute_cash(cash, &p).unwrap();
        prop_assert_eq!(r.total_distributed().unwrap(), cash);
        // Priority: senior coupon fully paid before any reserve dollar.
        if r.paid_reserve > 0 {
            prop_assert_eq!(r.unpaid_senior_coupon, 0);
        }
        // Reserve never overfilled beyond its deficit.
        prop_assert!(r.paid_reserve <= res_target.saturating_sub(res_cur));
    }

    // ---------------------------------------------------------------
    // Waterfall loss: hits sum to the loss exactly when absorbable;
    // over-stack losses are rejected, never silently truncated.
    // ---------------------------------------------------------------
    #[test]
    fn prop_loss_conservation(
        junior in 0u128..2_000_000,
        reserve in 0u128..1_000_000,
        senior in 0u128..4_000_000,
        frac in 0u32..=2_000,
    ) {
        let stack = junior + reserve + senior;
        // Sample loss across [0, stack] at 1/2000 granularity.
        let loss = stack * u128::from(frac) / 2_000;
        let r = apply_loss(loss, junior, reserve, senior).unwrap();
        prop_assert_eq!(r.junior_hit + r.reserve_hit + r.senior_hit, loss);
        prop_assert!(r.junior_hit <= junior);
        prop_assert!(r.reserve_hit <= reserve);
        prop_assert!(r.senior_hit <= senior);
        prop_assert_eq!(r.reserve_after, reserve - r.reserve_hit);
        // Over-stack always errors — explicitness is E12.
        prop_assert!(apply_loss(stack + 1, junior, reserve, senior).is_err());
    }

    // ---------------------------------------------------------------
    // PV: zero discount reproduces the raw schedule sum exactly;
    // discounts only reduce value; monotone in bps.
    // ---------------------------------------------------------------
    #[test]
    fn prop_pv_monotone_in_discount(
        qty in 1u128..10_000_000_000,        // ≤ 10K NPCS in base units
        price in 500_000u128..2_000_000,     // 0.5–2.0 USDC per NPCS
        periods in 0u32..6,
        bps in 0u64..5_000,
    ) {
        let fills: Vec<ExpectedFill> = (0..=periods)
            .map(|t| ExpectedFill { period: t, qty_base: qty, price_micros: price })
            .collect();
        let pv0 = pv_of_claim(&fills, 0).unwrap();
        let raw: u128 = fills
            .iter()
            .map(|f| f.qty_base * f.price_micros / 1_000_000)
            .sum();
        prop_assert_eq!(pv0, raw);
        let pv_bps = pv_of_claim(&fills, bps).unwrap();
        prop_assert!(pv_bps <= pv0);
        let pv_more = pv_of_claim(&fills, bps + 1).unwrap();
        prop_assert!(pv_more <= pv_bps);
    }

    // ---------------------------------------------------------------
    // Accrual: floored, monotone in time.
    // ---------------------------------------------------------------
    #[test]
    fn prop_accrual_floored_and_monotone(
        principal in 1u128..1_000_000_000_000,
        rate in 0u64..5_000,
        dt in 0u64..=31_536_000,
    ) {
        let i = accrue_interest(principal, rate, dt).unwrap();
        // Floor definition (exact value fits u128 at these bounds).
        let denom = 10_000u128 * 31_536_000u128;
        let exact = principal * u128::from(rate) * u128::from(dt);
        prop_assert!(i <= exact / denom);
        prop_assert!(exact / denom < i + 1);
        // Monotone in dt.
        let i2 = accrue_interest(principal, rate, dt + 1).unwrap();
        prop_assert!(i2 >= i);
    }

    // ---------------------------------------------------------------
    // Clearing is volume-maximal (AE-P5): brute force every candidate
    // price ≥ reserve and check nothing beats the returned volume. A
    // function returning the first crossing price would fail this.
    // ---------------------------------------------------------------
    #[test]
    fn prop_clearing_maximizes_executed_volume(
        orders in order_strategy(),
        reserve in 900_000u128..1_100_000,
        reference in 950_000u128..1_050_000,
    ) {
        let executed_at = |p: u128| -> u128 {
            let sell: u128 = orders.iter().filter(|o| !o.is_buy && o.limit_price <= p).map(|o| o.qty).sum();
            let buy: u128 = orders.iter().filter(|o| o.is_buy && o.limit_price >= p).map(|o| o.qty).sum();
            sell.min(buy)
        };
        let best = orders
            .iter()
            .map(|o| o.limit_price)
            .filter(|p| *p >= reserve)
            .map(executed_at)
            .max()
            .unwrap_or(0);
        match find_uniform_clear_price(&orders, reserve, reference).unwrap() {
            Some(r) => {
                prop_assert_eq!(r.executed_qty, best);
                prop_assert_eq!(executed_at(r.clear_price), r.executed_qty);
            }
            None => prop_assert_eq!(best, 0),
        }
    }

    // ---------------------------------------------------------------
    // Rounding sites round in their declared direction: Floor/Against-
    // Initiator never exceed the exact quotient, Ceil never falls below
    // it, and both stay within one unit of it.
    // ---------------------------------------------------------------
    #[test]
    fn prop_div_round_respects_direction(a in any::<u128>(), b in 1u128..=u128::MAX) {
        let f = a.div_round(b, RoundDirection::Floor).unwrap();
        let g = a.div_round(b, RoundDirection::AgainstInitiator).unwrap();
        let c = a.div_round(b, RoundDirection::Ceil).unwrap();
        prop_assert_eq!(f, g);
        prop_assert_eq!(f, a / b);
        prop_assert_eq!(c, ceil_div(a, b));
        prop_assert!(c == f || c == f + 1);
        prop_assert_eq!(c == f, a % b == 0);
    }
}
