//! Fuzz invariants for the cash paths (SRS 08 §11 audit scope, #125).
//!
//! Each `check_*` takes an `arbitrary` input, runs the **program's own**
//! code (`payment_rails::state`, `credit_math`) and panics on any broken
//! invariant. The libFuzzer targets in `fuzz_targets/` and the seeded smoke
//! test in `tests/smoke.rs` call the same functions, so stable CI and a
//! nightly fuzz run check one set of laws.
//!
//! Every input carries a `raw` flag. Raw inputs are used as drawn, so the
//! rejection paths get fuzzed; the rest are folded into the valid domain
//! (bps under their caps, amounts that fit), because uniformly random
//! integers almost never pass validation and would leave the Ok path,
//! where conservation lives, unexercised. Each `check_*` returns whether it
//! reached the Ok path; the smoke test fails if that gets rare.
//!
//! | Target | Code under test | Invariants |
//! |---|---|---|
//! | `settle_fill` | `FillSplit::compute` | legs sum to gross; gross is the exact ceiling; fee/reserve exact floors; Ok iff inputs are valid |
//! | `clearing` | `credit_math::find_uniform_clear_price` | price ≥ reserve and is a revealed limit; maximum volume, then the published tie-breaks, checked against a brute-force oracle; `None` only when nothing crosses |
//! | `pro_rata` | `credit_math::allocate_pro_rata` | sums to total; no bidder over its commitment; within 1 unit of exact pro-rata; Ok iff precondition holds; deterministic |
//! | `waterfall` | `distribute_cash`, `CouponSplit::compute`, `apply_loss` | exact sum; strict priority; loss absorbed bottom-up and exactly |
//! | `refund_bond` | `VaultDrain::compute`, `CashSession::refund_route` | vault drains exactly; a revealed bond is never forfeit; refunds always open at the hard deadline and never close again |
//! | `facility` | `Advance::max_advance` / `accrue` / `apply` | H1 ceiling; interest first; never takes more than owed; conservation |

#![deny(unsafe_code)]

use anchor_lang::prelude::Pubkey;
use arbitrary::Arbitrary;
use credit_math::{MathError, Order, WaterfallParams};
use payment_rails::state::{
    Advance, BondRoute, CashSession, CouponParams, CouponSplit, FillSplit, SessionStatus,
    VaultDrain, BPS, MAX_FEE_BPS, MAX_RESERVE_BPS,
};

// ── settle_fill ────────────────────────────────────────────────────────

/// One fill's inputs, unconstrained: invalid ones must be rejected, never
/// mis-split.
#[derive(Debug, Clone, Copy, Arbitrary)]
pub struct SettleFillInput {
    pub raw: bool,
    pub qty: u64,
    pub price: u64,
    pub asset_decimals: u8,
    pub fee_bps: u16,
    pub reserve_bps: u16,
}

/// AE-P8 cash split: `to_seller + fee + to_reserve == gross` and
/// `gross = ceil(qty × price / 10^dec)`, exactly, or a typed rejection.
pub fn check_settle_fill(mut i: SettleFillInput) -> bool {
    if !i.raw {
        // Fold into the valid domain: ≤ 2³² each, so gross fits u64.
        i.qty = i.qty >> 32 | 1;
        i.price = i.price >> 32 | 1;
        i.asset_decimals %= 19;
        i.fee_bps %= MAX_FEE_BPS + 1;
        i.reserve_bps %= MAX_RESERVE_BPS + 1;
    }
    let got = FillSplit::compute(i.qty, i.price, i.asset_decimals, i.fee_bps, i.reserve_bps);
    let unit = 10u128.checked_pow(u32::from(i.asset_decimals));
    let notional = u128::from(i.qty) * u128::from(i.price);
    let gross = unit.map(|u| notional.div_ceil(u));
    let valid = i.qty > 0
        && i.price > 0
        && i.fee_bps <= MAX_FEE_BPS
        && i.reserve_bps <= MAX_RESERVE_BPS
        && gross.is_some_and(|g| g <= u128::from(u64::MAX));
    let s = match (valid, got) {
        (true, Ok(s)) => s,
        (false, Err(_)) => return false,
        (v, g) => panic!("settle_fill: valid={v} but got {g:?} for {i:?}"),
    };
    let (unit, gross) = (unit.unwrap(), gross.unwrap());
    assert_eq!(u128::from(s.gross), gross, "gross is not the ceiling");
    // Ceiling: the buyer pays at least the notional, by less than one micro.
    // (gross − 1) × unit < notional can't overflow; gross × unit can, and
    // then it is certainly ≥ notional.
    assert!(gross.checked_mul(unit).is_none_or(|x| x >= notional));
    assert!((gross - 1) * unit < notional);
    assert_eq!(u128::from(s.fee), gross * u128::from(i.fee_bps) / BPS);
    assert_eq!(
        u128::from(s.to_reserve),
        gross * u128::from(i.reserve_bps) / BPS
    );
    assert_eq!(
        u128::from(s.to_seller) + u128::from(s.fee) + u128::from(s.to_reserve),
        u128::from(s.gross),
        "fill legs do not conserve gross"
    );
    true
}

// ── clearing ───────────────────────────────────────────────────────────

/// Raw orders are capped at 48 so one input stays cheap (clearing is
/// O(n²)). Non-raw inputs use `grid` instead: `arbitrary` draws Vec lengths
/// geometrically (half are empty), so a fixed book of 12 orders on a
/// 32-tick price grid is what makes crosses and ties common.
#[derive(Debug, Clone, Arbitrary)]
pub struct ClearingInput {
    pub raw: bool,
    pub orders: Vec<(u64, u128, u128, bool)>,
    pub grid: [(u8, u32, bool); 12],
    pub reserve_price: u128,
    pub reference_mark: u128,
}

/// AE-P5 uniform-price clearing against a brute-force oracle: among the
/// revealed limit prices ≥ reserve, maximum executed volume, then minimum
/// imbalance, then nearest the reference mark, then lowest price.
pub fn check_clearing(mut i: ClearingInput) -> bool {
    i.orders.truncate(48);
    if !i.raw {
        i.orders = (0u64..)
            .zip(i.grid)
            .map(|(id, (tick, qty, is_buy))| (id, u128::from(tick % 32), u128::from(qty), is_buy))
            .collect();
        i.reserve_price %= 32;
        i.reference_mark %= 40;
    }
    let orders: Vec<Order> = i
        .orders
        .iter()
        .map(|&(bidder_id, limit_price, qty, is_buy)| Order {
            bidder_id,
            limit_price,
            qty,
            is_buy,
        })
        .collect();
    let got = credit_math::find_uniform_clear_price(&orders, i.reserve_price, i.reference_mark);

    // Oracle: score every candidate; any overflowing side sum ⇒ Overflow.
    let side = |p: u128, buy: bool| {
        orders
            .iter()
            .filter(|o| {
                o.is_buy == buy
                    && if buy {
                        o.limit_price >= p
                    } else {
                        o.limit_price <= p
                    }
            })
            .try_fold(0u128, |a, o| a.checked_add(o.qty))
    };
    let mut best: Option<(u128, u128, u128, u128)> = None; // (exec, imb, dist, p)
    let mut overflow = false;
    for p in orders
        .iter()
        .map(|o| o.limit_price)
        .filter(|&p| p >= i.reserve_price)
    {
        let (Some(buy), Some(sell)) = (side(p, true), side(p, false)) else {
            overflow = true;
            continue;
        };
        let exec = buy.min(sell);
        if exec == 0 {
            continue;
        }
        let key = (exec, buy.abs_diff(sell), p.abs_diff(i.reference_mark), p);
        let better = best.is_none_or(|b| {
            (std::cmp::Reverse(key.0), key.1, key.2, key.3)
                < (std::cmp::Reverse(b.0), b.1, b.2, b.3)
        });
        if better {
            best = Some(key);
        }
    }
    match got {
        Err(e) => {
            assert!(
                overflow && e == MathError::Overflow,
                "clearing: {e:?} for {i:?}"
            );
            false
        }
        Ok(r) => {
            assert!(!overflow, "clearing ignored an overflow: {i:?}");
            match (r, best) {
                (None, None) => false,
                (Some(r), Some((exec, _, _, p))) => {
                    assert!(r.clear_price >= i.reserve_price, "cleared below reserve");
                    assert_eq!(
                        (r.clear_price, r.executed_qty),
                        (p, exec),
                        "not the best cross: {i:?}"
                    );
                    true
                }
                (r, b) => panic!("clearing: got {r:?}, oracle {b:?} for {i:?}"),
            }
        }
    }
}

// ── pro_rata ───────────────────────────────────────────────────────────

/// Bids are capped at 64 so one input stays cheap (allocation is O(n log n)).
#[derive(Debug, Clone, Arbitrary)]
pub struct ProRataInput {
    pub raw: bool,
    pub total: u128,
    pub bids: Vec<(u64, u128)>,
}

/// AE-P6 allocation: conservation, no over-allocation, ≤ 1 unit from exact.
pub fn check_pro_rata(mut i: ProRataInput) -> bool {
    i.bids.truncate(64);
    if !i.raw {
        // q < 2⁵⁶ and total ≤ Σq < 2⁶²: total × q cannot overflow.
        for b in &mut i.bids {
            b.1 >>= 72;
        }
    }
    let sum = i.bids.iter().try_fold(0u128, |a, &(_, q)| a.checked_add(q));
    if !i.raw {
        if let Some(s) = sum.filter(|s| *s > 0) {
            i.total %= s + 1;
        } else {
            i.total = 0;
        }
    }
    let got = credit_math::allocate_pro_rata(i.total, &i.bids);
    let expect_err = match sum {
        None => Some(MathError::Overflow),
        Some(0) if i.total == 0 => None,
        Some(0) => Some(MathError::InvalidInput),
        Some(s) if i.total > s => Some(MathError::OverAllocation),
        Some(_)
            if i.bids
                .iter()
                .any(|&(_, q)| i.total.checked_mul(q).is_none()) =>
        {
            Some(MathError::Overflow)
        }
        Some(_) => None,
    };
    let out = match (expect_err, got) {
        (Some(e), Err(g)) => {
            assert_eq!(e, g, "wrong rejection for {i:?}");
            return false;
        }
        (None, Ok(out)) => out,
        (e, g) => panic!("pro_rata: expected {e:?}, got {g:?} for {i:?}"),
    };
    let sum = sum.unwrap();
    assert_eq!(out.len(), i.bids.len());
    let mut allocated = 0u128;
    for (&(id, q), &(oid, a)) in i.bids.iter().zip(&out) {
        assert_eq!(id, oid, "input order not preserved");
        assert!(a <= q, "bidder {id} over-allocated: {a} > {q}");
        // Σq = 0 only with total = 0: everyone's exact share is 0.
        let floor = (i.total * q).checked_div(sum).unwrap_or(0);
        assert!(
            a == floor || a == floor + 1,
            "bidder {id}: {a} vs floor {floor}"
        );
        allocated += a;
    }
    assert_eq!(allocated, i.total, "allocation does not sum to total");
    // The published residue rule: the extra units go to the largest
    // commitments first, then the lower bidder_id (then input order). So
    // in that ranking the bidders that got +1 form a prefix.
    let mut rank: Vec<usize> = (0..i.bids.len()).collect();
    rank.sort_by(|&x, &y| {
        let (bx, by) = (i.bids[x], i.bids[y]);
        by.1.cmp(&bx.1).then(bx.0.cmp(&by.0))
    });
    let bumped = |k: usize| out[k].1 > (i.total * i.bids[k].1).checked_div(sum).unwrap_or(0);
    let n_bumped = rank.iter().filter(|&&k| bumped(k)).count();
    assert!(
        rank.iter().take(n_bumped).all(|&k| bumped(k)),
        "residue not handed out by (qty desc, bidder_id asc): {i:?}"
    );
    assert_eq!(
        credit_math::allocate_pro_rata(i.total, &i.bids).ok(),
        Some(out),
        "not deterministic"
    );
    true
}

// ── waterfall ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Arbitrary)]
pub struct WaterfallInput {
    pub raw: bool,
    pub cash: u64,
    pub fees_due: u64,
    pub senior_coupon_due: u64,
    pub reserve_target: u64,
    pub reserve_current: u64,
    pub senior_amort_due: u64,
    pub junior_coupon_due: u64,
    pub loss: u128,
    pub junior_principal: u128,
    pub senior_principal: u128,
}

/// TE-P4 cash waterfall (exact sum, strict priority) through both the
/// crate and the rails' `CouponSplit`, plus the bottom-up loss book.
pub fn check_waterfall(mut i: WaterfallInput) -> bool {
    if !i.raw {
        // A stack that fits, and a loss inside it.
        i.junior_principal >>= 2;
        i.senior_principal >>= 2;
        let stack = i.junior_principal + i.senior_principal + u128::from(i.reserve_current);
        i.loss %= stack + 1;
    }
    let p = WaterfallParams {
        fees_due: i.fees_due.into(),
        senior_coupon_due: i.senior_coupon_due.into(),
        reserve_target: i.reserve_target.into(),
        reserve_current: i.reserve_current.into(),
        senior_amort_due: i.senior_amort_due.into(),
        junior_coupon_due: i.junior_coupon_due.into(),
    };
    let cash = u128::from(i.cash);
    let w = credit_math::distribute_cash(cash, &p).expect("distribute_cash never fails");
    assert_eq!(w.total_distributed().unwrap(), cash, "waterfall leaks cash");
    let deficit = p.reserve_target.saturating_sub(p.reserve_current);
    // Strict priority: a line is paid only once every line above is whole.
    let lines = [
        (w.paid_fees, p.fees_due),
        (w.paid_senior_coupon, p.senior_coupon_due),
        (w.paid_reserve, deficit),
        (w.paid_senior_amort, p.senior_amort_due),
        (w.paid_junior_coupon, p.junior_coupon_due),
    ];
    let mut above_whole = true;
    for (n, &(paid, due)) in lines.iter().enumerate() {
        assert!(paid <= due, "line {n} overpaid");
        assert!(
            above_whole || paid == 0,
            "line {n} paid before a senior line"
        );
        above_whole &= paid == due;
    }
    assert!(
        above_whole || w.junior_residual == 0,
        "residual before lines"
    );
    assert_eq!(
        w.unpaid_senior_coupon,
        p.senior_coupon_due - w.paid_senior_coupon
    );
    assert_eq!(
        w.unpaid_senior_amort,
        p.senior_amort_due - w.paid_senior_amort
    );
    assert_eq!(
        w.unpaid_junior_coupon,
        p.junior_coupon_due - w.paid_junior_coupon
    );

    let c = CouponSplit::compute(
        i.cash,
        &CouponParams {
            fees_due: i.fees_due,
            senior_coupon_due: i.senior_coupon_due,
            reserve_target: i.reserve_target,
            reserve_current: i.reserve_current,
            senior_amort_due: i.senior_amort_due,
            junior_coupon_due: i.junior_coupon_due,
        },
    )
    .expect("CouponSplit::compute never fails on u64 inputs");
    assert_eq!(
        u128::from(c.fees) + u128::from(c.reserve) + u128::from(c.senior) + u128::from(c.junior),
        cash,
        "coupon split leaks cash"
    );
    assert_eq!(
        u128::from(c.senior),
        w.paid_senior_coupon + w.paid_senior_amort
    );

    // Loss: junior → reserve → senior, exactly, or rejected when it
    // exceeds the whole stack.
    let reserve = u128::from(i.reserve_current);
    let stack = i
        .junior_principal
        .checked_add(reserve)
        .and_then(|x| x.checked_add(i.senior_principal));
    let mut ok = false;
    match (
        stack,
        credit_math::apply_loss(i.loss, i.junior_principal, reserve, i.senior_principal),
    ) {
        (None, Err(MathError::Overflow)) => {}
        (Some(s), Err(MathError::InvalidInput)) if i.loss > s => {}
        (Some(s), Ok(l)) if i.loss <= s => {
            assert_eq!(l.junior_hit + l.reserve_hit + l.senior_hit, i.loss);
            assert!(l.junior_hit <= i.junior_principal);
            assert!(l.reserve_hit == 0 || l.junior_hit == i.junior_principal);
            assert!(l.senior_hit == 0 || l.reserve_hit == reserve);
            assert!(l.senior_hit <= i.senior_principal);
            assert_eq!(l.reserve_after, reserve - l.reserve_hit);
            ok = true;
        }
        (s, g) => panic!("apply_loss: stack {s:?} loss {} got {g:?}", i.loss),
    }
    ok
}

// ── refund_bond ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Arbitrary)]
pub struct RefundBondInput {
    pub raw: bool,
    /// What the vault actually holds (clawback / donation make it differ).
    pub held: u64,
    pub cash: u64,
    pub bond: u64,
    pub commit_end: i64,
    pub reveal_gap: u32,
    pub settle_gap: u32,
    /// 0 Open · 1 Failed · 2 Settled (mod 3).
    pub status: u8,
    pub failed_at: i64,
    pub revealed: bool,
    pub now: i64,
    pub later: u32,
}

fn session(i: &RefundBondInput) -> CashSession {
    let reveal_end = i.commit_end.saturating_add(i64::from(i.reveal_gap));
    CashSession {
        operator: Pubkey::default(),
        session_id: 0,
        usdc_mint: Pubkey::default(),
        reserve: Pubkey::default(),
        fee_receiver: Pubkey::default(),
        asset_mint: Pubkey::default(),
        asset_decimals: 6,
        fee_bps: 0,
        reserve_bps: 0,
        rofr_authority: Pubkey::default(),
        commit_end: i.commit_end,
        reveal_end,
        settle_deadline: reveal_end.saturating_add(i64::from(i.settle_gap)),
        bond_amount: i.bond,
        max_cash: u64::MAX,
        status: match i.status % 3 {
            0 => SessionStatus::Open,
            1 => SessionStatus::Failed,
            _ => SessionStatus::Settled,
        },
        failed_at: i.failed_at,
        open_escrows: 1,
        total_cash: i.cash,
        total_bonds: i.bond,
        total_asset: 0,
        fills_settled: 0,
        volume_settled: 0,
        gross_settled: 0,
        total_cash_shortfall: 0,
        total_asset_shortfall: 0,
        bump: 0,
    }
}

/// AE-P4 / AE-P10 / H5: who gets a refunding escrow's USDC, and when.
pub fn check_refund_bond(mut i: RefundBondInput) -> bool {
    if !i.raw {
        // A vault within a bond of what the books say (clawback or dust).
        i.cash >>= 2;
        i.bond >>= 2;
        let recorded = i.cash + i.bond;
        i.held = recorded.saturating_sub(i.bond) + i.held % (2 * i.bond + 1);
        // `now` near the session's windows, where the routes change.
        let span = i64::from(i.reveal_gap) + i64::from(i.settle_gap) + 2;
        i.commit_end >>= 2;
        i.now = i.commit_end + i.now.rem_euclid(span + 1) - 1;
        i.failed_at = i.commit_end + i.failed_at.rem_euclid(span + 1) - 1;
    }
    let s = session(&i);
    let route = s.refund_route(i.revealed, i.now);

    // Liveness: at or after the hard deadline every escrow is refundable,
    // and so is every escrow of a session that is no longer running.
    if i.now >= s.settle_deadline || s.status != SessionStatus::Open {
        assert!(route.is_ok(), "refund blocked: {i:?}");
    }
    // Once refundable, never locked again.
    if route.is_ok() {
        let later = i.now.saturating_add(i64::from(i.later));
        assert!(
            s.refund_route(i.revealed, later).is_ok(),
            "refund re-locked"
        );
    }
    // A revealed bidder's bond is never forfeit (AE-P4 targets no-shows),
    // and neither is anyone's before reveals close.
    if let Ok(r) = route {
        if i.revealed || i.now < s.reveal_end {
            assert_eq!(r, BondRoute::ToOwner, "bond forfeited wrongly: {i:?}");
        }
        if s.status == SessionStatus::Failed && s.failed_at < s.reveal_end {
            assert_eq!(
                r,
                BondRoute::ToOwner,
                "aborted-early session forfeited a bond"
            );
        }
    }

    // Draining the vault: exact, never blocked, gap booked.
    let recorded = i.cash.saturating_add(i.bond);
    let forfeit = match route {
        Ok(BondRoute::ToReserve) => i.bond,
        _ => 0,
    };
    let d = VaultDrain::compute(i.held, recorded, forfeit);
    assert_eq!(
        u128::from(d.to_reserve) + u128::from(d.to_owner),
        u128::from(i.held),
        "vault not drained exactly"
    );
    assert_eq!(d.to_reserve, forfeit.min(i.held), "forfeit not paid first");
    assert_eq!(d.shortfall, recorded.saturating_sub(i.held));
    if i.held >= recorded {
        assert_eq!(d.shortfall, 0);
        assert!(d.to_owner >= recorded - forfeit, "owner short-changed");
    }
    route.is_ok()
}

// ── facility ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Arbitrary)]
pub struct FacilityInput {
    pub raw: bool,
    pub collateral_value: u64,
    pub advance_rate_bps: u64,
    pub principal: u64,
    pub interest: u64,
    pub rate_bps: u16,
    pub last_accrual: i64,
    pub now: i64,
    pub pay: u64,
}

/// H1 ceiling, accrual monotonicity and interest-first repayment.
pub fn check_facility(mut i: FacilityInput) -> bool {
    if !i.raw {
        // Rates under H1, amounts and elapsed time (≤ ~34 years) that fit.
        i.advance_rate_bps %= credit_math::MAX_ADVANCE_RATE_BPS + 1;
        i.principal >>= 8;
        i.interest >>= 8;
        i.last_accrual >>= 2;
        i.now = i.last_accrual + i64::from(i.now as u32 >> 2) - 1_000;
    }
    match Advance::max_advance(i.collateral_value, i.advance_rate_bps) {
        Ok(cap) => {
            assert!(i.advance_rate_bps <= credit_math::MAX_ADVANCE_RATE_BPS);
            assert_eq!(
                u128::from(cap),
                u128::from(i.collateral_value) * u128::from(i.advance_rate_bps) / 10_000
            );
            assert!(
                cap <= i.collateral_value / 2,
                "H1: cap above 50% of the mark"
            );
        }
        Err(_) => assert!(i.advance_rate_bps > credit_math::MAX_ADVANCE_RATE_BPS),
    }

    let mut a = Advance {
        facility: Pubkey::default(),
        borrower: Pubkey::default(),
        collateral_ref_hash: [0; 32],
        collateral_value: i.collateral_value,
        advance_rate_bps: i.advance_rate_bps,
        principal: i.principal,
        interest: i.interest,
        last_accrual: i.last_accrual,
        bump: 0,
    };
    let before = a.clone();
    let rate = u64::from(i.rate_bps) % (payment_rails::state::MAX_RATE_BPS + 1);
    match a.accrue(rate, i.now) {
        Ok(()) => {
            assert_eq!(a.principal, before.principal, "accrual moved principal");
            assert!(a.interest >= before.interest, "interest went down");
            assert_eq!(
                a.last_accrual,
                before.last_accrual.max(i.now),
                "time ran backwards"
            );
            if i.now <= before.last_accrual {
                assert_eq!(a.interest, before.interest, "accrued over no time");
            }
        }
        // Only an overflow of the u64 interest counter may fail accrual.
        Err(_) => {
            let dt = u64::try_from(i.now.saturating_sub(i.last_accrual)).unwrap_or(0);
            let add = credit_math::accrue_interest(i.principal.into(), rate, dt);
            assert!(
                add.map_or(true, |x| x + u128::from(before.interest)
                    > u128::from(u64::MAX)),
                "accrual failed without overflow: {i:?}"
            );
            return false;
        }
    }

    let owed = u128::from(a.principal) + u128::from(a.interest);
    let (p0, i0) = (a.principal, a.interest);
    let r = a.apply(i.pay);
    let paid = u128::from(r.interest) + u128::from(r.principal);
    assert!(paid <= u128::from(i.pay), "took more than offered");
    assert_eq!(
        paid,
        u128::from(i.pay).min(owed),
        "did not take what was owed"
    );
    assert!(
        r.principal == 0 || r.interest == i0,
        "principal before interest"
    );
    assert_eq!(a.principal, p0 - r.principal);
    assert_eq!(a.interest, i0 - r.interest);
    assert_eq!(u128::from(r.total()), paid);
    true
}
