//! Allocation arithmetic — uniform-price clearing and pro-rata fills
//! (doc 09 §6; AE-P5/P6). Deterministic: same inputs → same outputs, always.

use crate::error::{MathError, RoundDirection};
use crate::RoundAwayFromInitiator;
use alloc::vec::Vec;

/// A sealed order after reveal. `is_buy` distinguishes demand from supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    /// Stable participant identifier (auction-engine maps it to a wallet).
    pub bidder_id: u64,
    /// Limit price in USDC micros per whole NPCS.
    pub limit_price: u128,
    /// Quantity in NPCS base units.
    pub qty: u128,
    /// true = buy side (USDC → NPCS), false = sell side.
    pub is_buy: bool,
}

/// A successful cross at a single uniform price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearResult {
    /// The single price every fill executes at (USDC micros per NPCS).
    pub clear_price: u128,
    /// Total base units that change hands.
    pub executed_qty: u128,
}

/// Uniform-price clearing (AE-P5): among candidate prices ≥ `reserve_price`,
/// find the one that maximizes executed volume; tie-break in order:
/// (1) minimal buy/sell imbalance, (2) nearest to `reference_mark`,
/// (3) lowest price. All deterministic — no reordering incentive (G11).
///
/// Returns `Ok(None)` when no candidate ≥ reserve crosses: the session goes
/// to `Failed` — never silently clears below reserve (A11).
///
/// Candidates are the revealed limit prices (a uniform-price equilibrium
/// price is always some participant's limit). O(n²) at pilot scale; the
/// prefix-sum refactor is a pure optimization when n grows (audit note).
pub fn find_uniform_clear_price(
    orders: &[Order],
    reserve_price: u128,
    reference_mark: u128,
) -> Result<Option<ClearResult>, MathError> {
    let mut candidates: Vec<u128> = orders
        .iter()
        .map(|o| o.limit_price)
        .filter(|p| *p >= reserve_price)
        .collect();
    candidates.sort_unstable();
    candidates.dedup();

    let mut best: Option<ClearResult> = None;
    let mut best_imbalance: u128 = 0;

    for &p in &candidates {
        let mut sell_qty: u128 = 0;
        let mut buy_qty: u128 = 0;
        for o in orders {
            if !o.is_buy {
                // Sellers participate at any price ≥ their limit.
                sell_qty = sell_qty
                    .checked_add(if o.limit_price <= p { o.qty } else { 0 })
                    .ok_or(MathError::Overflow)?;
            } else {
                // Buyers participate at any price ≤ their limit.
                buy_qty = buy_qty
                    .checked_add(if o.limit_price >= p { o.qty } else { 0 })
                    .ok_or(MathError::Overflow)?;
            }
        }
        let executed = sell_qty.min(buy_qty);
        if executed == 0 {
            continue;
        }
        let imbalance = sell_qty.abs_diff(buy_qty);

        let is_better = match &best {
            None => true,
            Some(b) => {
                if executed != b.executed_qty {
                    executed > b.executed_qty
                } else if imbalance != best_imbalance {
                    imbalance < best_imbalance
                } else {
                    let d_new = p.abs_diff(reference_mark);
                    let d_best = b.clear_price.abs_diff(reference_mark);
                    if d_new != d_best {
                        d_new < d_best
                    } else {
                        p < b.clear_price // final tie: lowest price
                    }
                }
            }
        };

        if is_better {
            best_imbalance = imbalance;
            best = Some(ClearResult {
                clear_price: p,
                executed_qty: executed,
            });
        }
    }
    Ok(best)
}

/// Pro-rata allocation at the margin (AE-P6): distribute `total` across
/// bidders proportionally to committed qty, **summing exactly to `total`**.
///
/// Algorithm (deterministic, no ordering advantage):
/// 1. everyone floors: `alloc_i = total × qty_i / Σqty` (bidders claim
///    units → round against them);
/// 2. the residue `total − Σalloc_i` (< n base units) is handed out one
///    unit at a time in order of (larger committed qty first, lower
///    bidder_id first) — a published rule, not a favor.
///
/// Precondition (checked, not assumed): `total ≤ Σqty`. Pro-rata can only
/// scale commitments **down** — a larger `total` would hand a bidder more
/// than they committed, so it is rejected with [`MathError::OverAllocation`].
/// Given the precondition, every `alloc_i ≤ qty_i`.
///
/// `bids` may be in any order; the returned vector preserves input order.
pub fn allocate_pro_rata(total: u128, bids: &[(u64, u128)]) -> Result<Vec<(u64, u128)>, MathError> {
    let sum_qty: u128 = bids
        .iter()
        .try_fold(0u128, |acc, &(_, q)| acc.checked_add(q))
        .ok_or(MathError::Overflow)?;
    // Preconditions only: every accepted input takes the single path below,
    // so the output is one entry per bid in input order, whatever the bids.
    if total > sum_qty {
        return Err(if sum_qty == 0 {
            // Nothing was committed at all: malformed input, not a scale-down.
            MathError::InvalidInput
        } else {
            MathError::OverAllocation
        });
    }

    let mut out: Vec<(u64, u128)> = bids
        .iter()
        .map(|&(id, q)| Ok((id, floor_share(total, q, sum_qty)?)))
        .collect::<Result<Vec<_>, MathError>>()?;

    let allocated: u128 = out.iter().map(|&(_, a)| a).sum();
    let residue = total.checked_sub(allocated).ok_or(MathError::Underflow)?;
    hand_out_residue(&mut out, bids, residue)?;
    Ok(out)
}

/// One bidder's floored share, `⌊total × qty / Σqty⌋` (bidders claim units:
/// round against them; the residue is re-attached by [`hand_out_residue`]).
///
/// Given `total ≤ Σqty`, a zero `total` is the only way `Σqty` can be 0, and
/// a zero total is a zero share for everyone, so no division by zero exists
/// on this path.
fn floor_share(total: u128, qty: u128, sum_qty: u128) -> Result<u128, MathError> {
    if total == 0 {
        return Ok(0);
    }
    total
        .checked_mul(qty)
        .ok_or(MathError::Overflow)?
        .div_round(sum_qty, RoundDirection::AgainstInitiator)
}

/// Hand `residue` out one unit per bidder, larger committed qty first, then
/// lower bidder_id. `out` and `bids` are index-aligned.
///
/// Flooring loses < 1 unit per bidder, so from [`allocate_pro_rata`] the
/// residue is always < n and drains. A residue that does not drain is
/// rejected rather than dropped (crate rule 4: exact conservation), in
/// release builds too. Split out so that guard is tested directly (#87).
fn hand_out_residue(
    out: &mut [(u64, u128)],
    bids: &[(u64, u128)],
    mut residue: u128,
) -> Result<(), MathError> {
    let mut order: Vec<usize> = (0..bids.len()).collect();
    order.sort_by(|&i, &j| bids[j].1.cmp(&bids[i].1).then(bids[i].0.cmp(&bids[j].0)));
    for &i in &order {
        if residue == 0 {
            break;
        }
        out[i].1 += 1;
        residue -= 1;
    }
    if residue != 0 {
        return Err(MathError::InvalidInput);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const O: fn(u64, u128, u128, bool) -> Order = |bidder_id, limit_price, qty, is_buy| Order {
        bidder_id,
        limit_price,
        qty,
        is_buy,
    };

    #[test]
    fn clears_at_max_volume_price() {
        // Sells: 1000 @ 985_000, 600 @ 995_000
        // Buys:   700 @ 1_005_000, 900 @ 990_000
        // p=985_000: sell 1000, buy 1600 → exec 1000 (imb 600)
        // p=990_000: sell 1000, buy 1600 → exec 1000 (imb 600, nearer ref)
        // p=995_000: sell 1600, buy 700  → exec 700  (900@990k is out of the money)
        // p=1_005_000: sell 1600, buy 700 → exec 700
        let orders = [
            O(1, 985_000, 1_000, false),
            O(2, 995_000, 600, false),
            O(3, 1_005_000, 700, true),
            O(4, 990_000, 900, true),
        ];
        let r = find_uniform_clear_price(&orders, 900_000, 992_000)
            .unwrap()
            .unwrap();
        assert_eq!(r.clear_price, 990_000);
        assert_eq!(r.executed_qty, 1_000);
    }

    #[test]
    fn tie_break_prefers_minimal_imbalance_then_reference() {
        // exec ties at 1000 for both prices; imb ties at 600;
        // reference 992_000 → 990_000 is nearer.
        let orders = [
            O(1, 985_000, 1_000, false),
            O(3, 1_005_000, 700, true),
            O(4, 990_000, 900, true),
        ];
        let r = find_uniform_clear_price(&orders, 900_000, 992_000)
            .unwrap()
            .unwrap();
        assert_eq!(r.clear_price, 990_000);
        assert_eq!(r.executed_qty, 1_000);
    }

    #[test]
    fn tie_break_equidistant_from_reference_takes_lowest_price() {
        // 99 and 101 both execute 10 with zero imbalance and sit 1 either
        // side of the reference 100: the final rule picks the lower price.
        // 105 is a candidate that crosses nothing (no buyer at 105).
        let orders = [
            O(1, 99, 10, false),
            O(2, 101, 10, true),
            O(3, 105, 5, false),
        ];
        let r = find_uniform_clear_price(&orders, 0, 100).unwrap().unwrap();
        assert_eq!(r.clear_price, 99);
        assert_eq!(r.executed_qty, 10);
    }

    #[test]
    fn tie_break_prefers_lower_imbalance() {
        // p=100: sell 10, buy 25 → exec 10, imb 15.
        // p=103: sell 30, buy 10 → exec 10, imb 20. Reference favours 103,
        // but imbalance is compared first.
        let orders = [
            O(1, 100, 10, false),
            O(2, 103, 20, false),
            O(3, 103, 10, true),
            O(4, 100, 15, true),
        ];
        let r = find_uniform_clear_price(&orders, 0, 103).unwrap().unwrap();
        assert_eq!(r.clear_price, 100);
        assert_eq!(r.executed_qty, 10);
    }

    #[test]
    fn reserve_blocks_cross() {
        // Best possible exec above reserve 1_000_000: none (all bids below).
        let orders = [O(1, 990_000, 1_000, false), O(2, 995_000, 500, true)];
        let r = find_uniform_clear_price(&orders, 1_000_000, 995_000).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn pro_rata_sums_exactly() {
        let r = allocate_pro_rata(999, &[(1, 1_000), (2, 1_000), (3, 1_000)]).unwrap();
        assert_eq!(r.iter().map(|&(_, a)| a).sum::<u128>(), 999);
        assert!(r.iter().all(|&(_, a)| a == 333));
    }

    #[test]
    fn pro_rata_residue_goes_to_larger_commit() {
        // total 99 over {30, 70}: floors {29, 69}, residue 1 → qty-70 bidder.
        let r = allocate_pro_rata(99, &[(5, 30), (1, 70)]).unwrap();
        assert_eq!(r, vec![(5, 29), (1, 70)]);
    }

    #[test]
    fn pro_rata_tiny_total_dust_order() {
        // total 2 over {10, 10, 5}: floors all 0; residue 2 → the two
        // qty-10 bidders, tie broken by lower id (4 before 9).
        let r = allocate_pro_rata(2, &[(9, 10), (4, 10), (7, 5)]).unwrap();
        assert_eq!(r, vec![(9, 1), (4, 1), (7, 0)]);
    }

    #[test]
    fn pro_rata_rejects_zero_sum_against_nonzero_total() {
        assert_eq!(
            allocate_pro_rata(100, &[(1, 0)]).unwrap_err(),
            MathError::InvalidInput
        );
        assert_eq!(allocate_pro_rata(0, &[]).unwrap(), vec![]);
        // Zero-qty bids still get an entry each (found by the fuzz target).
        assert_eq!(
            allocate_pro_rata(0, &[(1, 0), (2, 0)]).unwrap(),
            vec![(1, 0), (2, 0)]
        );
    }

    #[test]
    fn pro_rata_rejects_total_above_committed() {
        // One bidder commits 1 unit; allocating 100 would over-allocate 100×.
        assert_eq!(
            allocate_pro_rata(100, &[(1, 1)]).unwrap_err(),
            MathError::OverAllocation
        );
        assert_eq!(
            allocate_pro_rata(301, &[(1, 100), (2, 100), (3, 100)]).unwrap_err(),
            MathError::OverAllocation
        );
    }

    #[test]
    fn pro_rata_total_equal_to_committed_passes_through() {
        let bids = [(3, 7), (1, 250), (2, 1)];
        assert_eq!(allocate_pro_rata(258, &bids).unwrap(), bids.to_vec());
    }

    #[test]
    fn pro_rata_product_overflow_is_typed() {
        // total × qty_i = 2²⁵⁴ does not fit u128: Overflow, never a wrap.
        assert_eq!(
            allocate_pro_rata(1 << 127, &[(1, 1 << 127), (2, 1 << 126)]).unwrap_err(),
            MathError::Overflow
        );
    }

    #[test]
    fn residue_that_cannot_drain_is_rejected_not_dropped() {
        // Two bidders absorb at most 2 units; a residue of 3 must fail.
        let bids = [(1, 5), (2, 5)];
        let mut out = vec![(1, 0), (2, 0)];
        assert_eq!(
            hand_out_residue(&mut out, &bids, 3).unwrap_err(),
            MathError::InvalidInput
        );
        let mut out = vec![(1, 0), (2, 0)];
        hand_out_residue(&mut out, &bids, 2).unwrap();
        assert_eq!(out, vec![(1, 1), (2, 1)]);
    }
}
