//! R2 exit criteria (PHASES.md): settlement conserves every micro-USDC (and
//! every asset unit); a replayed fill is rejected; AC-3 cash leg green.
//! Every test runs with the settlement mint under SPL Token and Token-2022;
//! the asset is always Token-2022 (like NPCS), so the mixed case is covered.

mod common;

use common::*;
use payment_rails::{Breaker, FillSplit, RailsError, SessionStatus, MAX_FEE_BPS, MAX_RESERVE_BPS};
use solana_signer::Signer;

const FEE_BPS: u16 = 10;
const RES_BPS: u16 = 5;
const UNIT: u64 = 1_000_000; // 1 whole asset (6 dp)
const PRICE: u64 = 98_123_456; // 98.123456 USDC per whole unit

fn each(test: impl Fn(Tp)) {
    for tp in Tp::ALL {
        test(tp);
    }
}

fn err(e: RailsError) -> Fail {
    Fail::rails(e)
}

fn split(qty: u64, price: u64) -> FillSplit {
    FillSplit::compute(qty, price, 6, FEE_BPS, RES_BPS).unwrap()
}

/// Session with fees set, `sellers` asks and `buyers` bids deposited and
/// all revealed; clock parked at `REVEAL_END` (settlement open).
struct Book {
    env: Env,
    s: anchor_lang::prelude::Pubkey,
    sellers: Vec<Party>,
    buyers: Vec<Party>,
}

fn book(tp: Tp, asks: &[u64], bids: &[u64]) -> Book {
    let mut env = Env::new(tp);
    env.set_fees(FEE_BPS, RES_BPS).unwrap();
    let s = env.open_session(1);
    let sellers: Vec<Party> = asks
        .iter()
        .map(|&q| {
            let p = env.party(BOND, q);
            env.deposit_ask(&s, &p, q).unwrap();
            p
        })
        .collect();
    let buyers: Vec<Party> = bids
        .iter()
        .map(|&c| {
            let p = env.party(c + BOND, 0);
            env.deposit(&s, &p.key, &p.usdc, c).unwrap();
            p
        })
        .collect();
    env.set_time(COMMIT_END);
    for p in sellers.iter().chain(&buyers) {
        env.mark_revealed(&s, &p.key.pubkey()).unwrap();
    }
    env.set_time(REVEAL_END);
    Book {
        env,
        s,
        sellers,
        buyers,
    }
}

// ── conservation (R2 exit criterion) ────────────────────────────────────

/// Two sellers, two buyers, three fills at a non-round price with fees,
/// then finish + refunds. After every step the USDC total and the asset
/// total are unchanged; at the end every party holds exactly what the
/// fills say, fee/reserve hold exactly the summed splits, and the session
/// is fully drained.
#[test]
fn dvp_conserves_every_micro_usdc_and_asset_unit() {
    each(|tp| {
        let Book {
            mut env,
            s,
            sellers,
            buyers,
        } = book(tp, &[UNIT, 2 * UNIT], &[300_000_000, 150_000_000]);
        let (a, b) = (&sellers[0], &sellers[1]);
        let (x, y) = (&buyers[0], &buyers[1]);

        let mut usdc = vec![env.fee, env.reserve];
        let mut asset = vec![];
        for p in sellers.iter().chain(&buyers) {
            usdc.push(p.usdc);
            asset.push(p.asset);
            let e = escrow_pda(&s, &p.key.pubkey());
            usdc.push(vault_pda(&e));
            asset.push(asset_vault_pda(&e));
        }
        let (usdc0, asset0) = (env.total(&usdc), env.total(&asset));

        let fills = [
            (x, a, UNIT),         // X ← A 1.0
            (x, b, UNIT * 3 / 2), // X ← B 1.5
            (y, b, UNIT / 2),     // Y ← B 0.5
        ];
        for (i, (buyer, seller, qty)) in fills.iter().enumerate() {
            let f = env.fill(&s, buyer, seller);
            env.settle(&f, i as u32, *qty, PRICE).unwrap();
            assert_eq!(
                env.total(&usdc),
                usdc0,
                "{tp:?} USDC conserved after fill {i}"
            );
            assert_eq!(
                env.total(&asset),
                asset0,
                "{tp:?} asset conserved after fill {i}"
            );
            let r = env.receipt(&s, i as u32);
            assert_eq!(r.split, split(*qty, PRICE), "{tp:?} receipt split");
            assert_eq!((r.qty, r.price, r.rofr), (*qty, PRICE, false));
        }

        let splits: Vec<FillSplit> = fills.iter().map(|(_, _, q)| split(*q, PRICE)).collect();
        let sum = |f: fn(&FillSplit) -> u64| splits.iter().map(f).sum::<u64>();
        let sess = env.session(&s);
        assert_eq!(sess.fills_settled, 3);
        assert_eq!(sess.volume_settled, 3 * UNIT);
        assert_eq!(sess.gross_settled, sum(|f| f.gross));
        assert_eq!(sess.total_asset, 0, "all asks fully sold");

        // Refunds stay locked until the operator finishes (or the deadline).
        assert_eq!(
            env.refund(&x.key, &s, &x.key.pubkey(), &x.usdc)
                .unwrap_err(),
            err(RailsError::DeadlineNotReached)
        );
        env.finish_session(&s).unwrap();
        assert_eq!(env.session(&s).status, SessionStatus::Settled);
        let crank = env.funded();
        for p in &buyers {
            env.refund(&crank, &s, &p.key.pubkey(), &p.usdc).unwrap();
        }
        for p in &sellers {
            env.refund_ask(&crank, &s, p).unwrap();
        }
        assert_eq!(env.total(&usdc), usdc0, "{tp:?} USDC conserved end to end");
        assert_eq!(
            env.total(&asset),
            asset0,
            "{tp:?} asset conserved end to end"
        );

        // Exact end state.
        let (gx, gy) = (splits[0].gross + splits[1].gross, splits[2].gross);
        assert_eq!(env.bal(&x.usdc), 300_000_000 + BOND - gx, "X paid exactly");
        assert_eq!(env.bal(&y.usdc), 150_000_000 + BOND - gy, "Y paid exactly");
        assert_eq!(env.bal(&x.asset), UNIT * 5 / 2);
        assert_eq!(env.bal(&y.asset), UNIT / 2);
        assert_eq!(env.bal(&a.usdc), BOND + splits[0].to_seller);
        assert_eq!(
            env.bal(&b.usdc),
            BOND + splits[1].to_seller + splits[2].to_seller
        );
        assert_eq!((env.bal(&a.asset), env.bal(&b.asset)), (0, 0));
        assert_eq!(env.bal(&env.fee), sum(|f| f.fee), "{tp:?} fees exact");
        assert_eq!(
            env.bal(&env.reserve),
            sum(|f| f.to_reserve),
            "{tp:?} reserve exact"
        );
        let sess = env.session(&s);
        assert_eq!(
            (
                sess.open_escrows,
                sess.total_cash,
                sess.total_bonds,
                sess.total_asset
            ),
            (0, 0, 0, 0),
            "{tp:?} session drained"
        );
    });
}

// ── idempotency (R2 exit criterion) ─────────────────────────────────────

#[test]
fn replayed_fill_is_rejected_and_pays_nothing() {
    each(|tp| {
        let Book {
            mut env,
            s,
            sellers,
            buyers,
        } = book(tp, &[2 * UNIT], &[500_000_000]);
        let f = env.fill(&s, &buyers[0], &sellers[0]);
        env.settle(&f, 7, UNIT, PRICE).unwrap();
        let snapshot = (
            env.bal(&sellers[0].usdc),
            env.bal(&buyers[0].asset),
            env.bal(&env.fee),
            env.session(&s).fills_settled,
        );
        // Same index again — even with different numbers — must not pay.
        assert!(env.settle(&f, 7, UNIT, PRICE).is_err(), "{tp:?} replay");
        assert!(
            env.settle(&f, 7, UNIT / 2, PRICE / 2).is_err(),
            "{tp:?} replay, new params"
        );
        assert_eq!(
            (
                env.bal(&sellers[0].usdc),
                env.bal(&buyers[0].asset),
                env.bal(&env.fee),
                env.session(&s).fills_settled
            ),
            snapshot,
            "{tp:?} nothing moved"
        );
        // A genuinely new fill index still settles.
        env.settle(&f, 8, UNIT, PRICE).unwrap();
        assert_eq!(env.session(&s).fills_settled, 2);
    });
}

// ── timing & halts ──────────────────────────────────────────────────────

#[test]
fn settlement_only_inside_its_window() {
    each(|tp| {
        let Book {
            mut env,
            s,
            sellers,
            buyers,
        } = book(tp, &[UNIT], &[500_000_000]);
        let f = env.fill(&s, &buyers[0], &sellers[0]);
        env.set_time(REVEAL_END - 1);
        assert_eq!(
            env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
            err(RailsError::OutsideWindow),
            "{tp:?}"
        );
        env.set_time(SETTLE_DEADLINE);
        assert_eq!(
            env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
            err(RailsError::OutsideWindow),
            "{tp:?}"
        );
        env.set_time(REVEAL_END);
        env.finish_session(&s).unwrap();
        assert_eq!(
            env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
            err(RailsError::SessionNotOpen)
        );
        assert_eq!(
            env.finish_session(&s).unwrap_err(),
            err(RailsError::SessionNotOpen)
        );
    });
}

#[test]
fn finish_only_after_reveals_close() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        env.set_time(COMMIT_END);
        assert_eq!(
            env.finish_session(&s).unwrap_err(),
            err(RailsError::OutsideWindow),
            "{tp:?}"
        );
        let rogue = env.funded();
        env.set_time(REVEAL_END);
        assert_eq!(
            env.finish_session_as(&rogue, &s).unwrap_err(),
            err(RailsError::NotOperator)
        );
    });
}

/// Depeg breaker (and pause) halt settlement — but never refunds.
#[test]
fn depeg_breaker_halts_settlement_not_refunds() {
    each(|tp| {
        let Book {
            mut env,
            s,
            sellers,
            buyers,
        } = book(tp, &[UNIT], &[500_000_000]);
        let f = env.fill(&s, &buyers[0], &sellers[0]);
        env.set_breaker(Breaker::Depeg, true).unwrap();
        assert_eq!(
            env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
            err(RailsError::DepegBreakerTripped),
            "{tp:?}"
        );
        env.set_breaker(Breaker::Pause, true).unwrap();
        assert_eq!(
            env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
            err(RailsError::Paused)
        );

        // Hard deadline: both sides get everything back, breaker still on.
        env.set_time(SETTLE_DEADLINE);
        let stranger = env.funded();
        env.refund(&stranger, &s, &buyers[0].key.pubkey(), &buyers[0].usdc)
            .unwrap();
        env.refund_ask(&stranger, &s, &sellers[0]).unwrap();
        assert_eq!(
            env.bal(&buyers[0].usdc),
            500_000_000 + BOND,
            "{tp:?} buyer whole"
        );
        assert_eq!(env.bal(&sellers[0].asset), UNIT, "{tp:?} seller asset back");
        assert_eq!(env.bal(&sellers[0].usdc), BOND, "{tp:?} seller bond back");
    });
}

// ── what the operator can and cannot do ─────────────────────────────────

#[test]
fn fills_need_revealed_opposite_sides_and_enough_escrow() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let seller = env.party(BOND, UNIT);
        let buyer = env.party(200_000_000 + BOND, 0);
        let buyer2 = env.party(200_000_000 + BOND, 0);
        env.deposit_ask(&s, &seller, UNIT).unwrap();
        env.deposit(&s, &buyer.key, &buyer.usdc, 200_000_000)
            .unwrap();
        env.deposit(&s, &buyer2.key, &buyer2.usdc, 200_000_000)
            .unwrap();
        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &seller.key.pubkey()).unwrap();
        env.mark_revealed(&s, &buyer.key.pubkey()).unwrap();
        env.set_time(REVEAL_END);

        // buyer2 never revealed.
        let f2 = env.fill(&s, &buyer2, &seller);
        assert_eq!(
            env.settle(&f2, 0, UNIT, PRICE).unwrap_err(),
            err(RailsError::NotRevealed),
            "{tp:?}"
        );
        // Two buy escrows can't trade, nor can the sides be swapped. A buy
        // escrow has no asset vault, so account validation rejects these
        // before the handler's `WrongSide` check (kept as defence in depth).
        let bb = env.fill(&s, &buyer, &buyer2);
        assert!(
            env.settle(&bb, 0, UNIT, PRICE).is_err(),
            "{tp:?} buy vs buy"
        );
        let sb = env.fill(&s, &seller, &buyer);
        assert!(
            env.settle(&sb, 0, UNIT, PRICE).is_err(),
            "{tp:?} swapped sides"
        );
        assert!(
            !env.exists(&receipt_pda(&s, 0)),
            "{tp:?} no receipt written"
        );

        let f = env.fill(&s, &buyer, &seller);
        // More asset than escrowed.
        assert_eq!(
            env.settle(&f, 0, UNIT + 1, 1).unwrap_err(),
            err(RailsError::InsufficientEscrow)
        );
        // More cash than escrowed: 1 unit at 200.000001 USDC.
        assert_eq!(
            env.settle(&f, 0, UNIT, 200_000_001).unwrap_err(),
            err(RailsError::InsufficientEscrow)
        );
        assert_eq!(
            env.settle(&f, 0, 0, PRICE).unwrap_err(),
            err(RailsError::ZeroAmount)
        );
        assert_eq!(
            env.settle(&f, 0, UNIT, 0).unwrap_err(),
            err(RailsError::ZeroAmount)
        );
        // Exactly the whole escrow is fine.
        env.settle(&f, 0, UNIT, 200_000_000).unwrap();
        assert_eq!(env.escrow(&s, &buyer.key.pubkey()).cash, 0);
        assert_eq!(env.escrow(&s, &seller.key.pubkey()).asset, 0);
    });
}

/// Every settlement destination is pinned: the operator (or anyone
/// assembling the accounts) cannot route value outside the fill's parties
/// and the snapshotted fee/reserve accounts.
#[test]
fn operator_cannot_redirect_any_leg() {
    each(|tp| {
        let Book {
            mut env,
            s,
            sellers,
            buyers,
        } = book(tp, &[UNIT], &[500_000_000]);
        let thief = env.party(0, 0);
        let mint = env.mint;
        let thief_usdc2 = env.token_account(&mint, &thief.key.pubkey(), 0);
        let good = env.fill(&s, &buyers[0], &sellers[0]);

        let cases = [
            (
                Fill {
                    seller_usdc: thief.usdc,
                    ..good
                },
                RailsError::NotEscrowOwner,
            ),
            (
                Fill {
                    buyer_asset: thief.asset,
                    ..good
                },
                RailsError::NotEscrowOwner,
            ),
            (
                Fill {
                    fee_receiver: thief_usdc2,
                    ..good
                },
                RailsError::FeeReceiverMismatch,
            ),
            (
                Fill {
                    reserve: thief_usdc2,
                    ..good
                },
                RailsError::ReserveMismatch,
            ),
            // Paying the seller's USDC into the asset slot and vice versa.
            (
                Fill {
                    buyer_asset: buyers[0].usdc,
                    ..good
                },
                RailsError::AssetMismatch,
            ),
            (
                Fill {
                    seller_usdc: sellers[0].asset,
                    ..good
                },
                RailsError::MintMismatch,
            ),
        ];
        for (i, (f, want)) in cases.iter().enumerate() {
            assert_eq!(
                env.settle(f, 0, UNIT, PRICE).unwrap_err(),
                err(*want),
                "{tp:?} case {i}"
            );
        }
        let rogue = env.funded();
        assert_eq!(
            env.settle_as(&rogue, &good, 0, UNIT, PRICE).unwrap_err(),
            err(RailsError::NotOperator)
        );

        env.settle(&good, 0, UNIT, PRICE).unwrap();
        assert_eq!(
            env.bal(&thief.usdc) + env.bal(&thief.asset) + env.bal(&thief_usdc2),
            0,
            "{tp:?}"
        );
    });
}

// ── AE-P7 ROFR cash path ────────────────────────────────────────────────

#[test]
fn issuer_rofr_pays_cash_and_takes_delivery() {
    each(|tp| {
        let mut env = Env::new(tp);
        env.set_fees(FEE_BPS, RES_BPS).unwrap();
        let issuer = env.party(1_000_000_000, 0);
        let s = env.open_session_with_rofr(1, issuer.key.pubkey());
        let seller = env.party(BOND, 2 * UNIT);
        env.deposit_ask(&s, &seller, 2 * UNIT).unwrap();
        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &seller.key.pubkey()).unwrap();
        env.set_time(REVEAL_END);

        let impostor = env.party(1_000_000_000, 0);
        assert_eq!(
            env.settle_rofr(&impostor, &s, &seller, 0, UNIT, PRICE)
                .unwrap_err(),
            err(RailsError::NotRofrAuthority),
            "{tp:?}"
        );

        let usdc = [issuer.usdc, seller.usdc, env.fee, env.reserve];
        let before = env.total(&usdc);
        env.settle_rofr(&issuer, &s, &seller, 0, UNIT, PRICE)
            .unwrap();
        let sp = split(UNIT, PRICE);
        assert_eq!(
            env.bal(&issuer.usdc),
            1_000_000_000 - sp.gross,
            "{tp:?} issuer paid gross"
        );
        assert_eq!(env.bal(&issuer.asset), UNIT, "{tp:?} issuer took delivery");
        assert_eq!(env.bal(&seller.usdc), sp.to_seller);
        assert_eq!(env.bal(&env.fee), sp.fee);
        assert_eq!(env.bal(&env.reserve), sp.to_reserve);
        assert_eq!(env.total(&usdc), before, "{tp:?} conserved");
        let r = env.receipt(&s, 0);
        assert!(r.rofr);
        assert_eq!(r.buyer, issuer.key.pubkey());
        // Fill indices are shared with normal fills: replay rejected.
        assert!(env
            .settle_rofr(&issuer, &s, &seller, 0, UNIT, PRICE)
            .is_err());
        // Remaining ask comes back after finish.
        env.finish_session(&s).unwrap();
        let crank = env.funded();
        env.refund_ask(&crank, &s, &seller).unwrap();
        assert_eq!(env.bal(&seller.asset), UNIT, "{tp:?} unsold half returned");
    });
}

#[test]
fn rofr_disabled_unless_configured() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1); // rofr_authority = default
        let seller = env.party(BOND, UNIT);
        env.deposit_ask(&s, &seller, UNIT).unwrap();
        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &seller.key.pubkey()).unwrap();
        env.set_time(REVEAL_END);
        let anyone = env.party(1_000_000_000, 0);
        assert_eq!(
            env.settle_rofr(&anyone, &s, &seller, 0, UNIT, PRICE)
                .unwrap_err(),
            err(RailsError::NotRofrAuthority),
            "{tp:?}"
        );
    });
}

// ── sell-side escrow lifecycle ──────────────────────────────────────────

#[test]
fn ask_escrow_refunds_and_side_guards() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let honest = env.party(BOND, UNIT);
        let ghost = env.party(BOND, UNIT); // never reveals
        let buyer = env.party(100_000_000 + BOND, 0);
        env.deposit_ask(&s, &honest, UNIT).unwrap();
        env.deposit_ask(&s, &ghost, UNIT).unwrap();
        env.deposit(&s, &buyer.key, &buyer.usdc, 100_000_000)
            .unwrap();
        assert_eq!(env.session(&s).total_asset, 2 * UNIT);

        // Asset dust donated to a vault can't block its close.
        let griefer = env.party(0, 10);
        let av = asset_vault_pda(&escrow_pda(&s, &honest.key.pubkey()));
        env.raw_asset_transfer(&griefer.asset, &av, &griefer.key, 1)
            .unwrap();

        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &honest.key.pubkey()).unwrap();
        env.set_time(REVEAL_END);
        let crank = env.funded();

        // Side guards: each refund path only accepts its own side.
        assert_eq!(
            env.refund(&crank, &s, &ghost.key.pubkey(), &ghost.usdc)
                .unwrap_err(),
            err(RailsError::WrongSide),
            "{tp:?}"
        );
        // A buy escrow has no asset vault: `has_one = asset_vault` rejects.
        assert!(env.refund_ask(&crank, &s, &buyer).is_err(), "{tp:?}");

        // Unrevealed seller: asset back, bond forfeited.
        env.refund_ask(&crank, &s, &ghost).unwrap();
        assert_eq!(env.bal(&ghost.asset), UNIT, "{tp:?} ghost asset back");
        assert_eq!(env.bal(&ghost.usdc), 0, "{tp:?} ghost bond forfeited");
        assert_eq!(env.bal(&env.reserve), BOND);

        // Revealed seller waits for finish, then gets asset (+dust) + bond.
        assert_eq!(
            env.refund_ask(&crank, &s, &honest).unwrap_err(),
            err(RailsError::DeadlineNotReached)
        );
        env.finish_session(&s).unwrap();
        env.refund_ask(&crank, &s, &honest).unwrap();
        assert_eq!(env.bal(&honest.asset), UNIT + 1, "{tp:?} asset + dust");
        assert_eq!(env.bal(&honest.usdc), BOND);
        let e = escrow_pda(&s, &honest.key.pubkey());
        assert!(
            !env.exists(&e) && !env.exists(&vault_pda(&e)) && !env.exists(&asset_vault_pda(&e))
        );

        // Ask refund destinations are pinned to the seller too.
        env.refund(&crank, &s, &buyer.key.pubkey(), &buyer.usdc)
            .unwrap();
        assert_eq!(env.session(&s).open_escrows, 0);
        assert_eq!(env.session(&s).total_asset, 0);
    });
}

#[test]
fn ask_refund_cannot_be_redirected() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let seller = env.party(BOND, UNIT);
        env.deposit_ask(&s, &seller, UNIT).unwrap();
        env.set_time(SETTLE_DEADLINE);
        let thief = env.party(0, 0);
        let spoof_asset = Party {
            key: seller.key.insecure_clone(),
            usdc: seller.usdc,
            asset: thief.asset,
        };
        let spoof_usdc = Party {
            key: seller.key.insecure_clone(),
            usdc: thief.usdc,
            asset: seller.asset,
        };
        assert_eq!(
            env.refund_ask(&thief.key, &s, &spoof_asset).unwrap_err(),
            err(RailsError::NotEscrowOwner),
            "{tp:?}"
        );
        assert_eq!(
            env.refund_ask(&thief.key, &s, &spoof_usdc).unwrap_err(),
            err(RailsError::NotEscrowOwner)
        );
        env.refund_ask(&thief.key, &s, &seller).unwrap();
        assert_eq!(env.bal(&seller.asset), UNIT);
    });
}

// ── fees & session parameters ───────────────────────────────────────────

#[test]
fn fees_are_capped_governed_and_snapshotted() {
    each(|tp| {
        let mut env = Env::new(tp);
        assert_eq!(
            env.set_fees(MAX_FEE_BPS + 1, 0).unwrap_err(),
            err(RailsError::FeeTooHigh),
            "{tp:?}"
        );
        assert_eq!(
            env.set_fees(0, MAX_RESERVE_BPS + 1).unwrap_err(),
            err(RailsError::FeeTooHigh)
        );
        let rogue = env.funded();
        assert_eq!(
            env.set_fees_as(&rogue, 1, 1).unwrap_err(),
            err(RailsError::Unauthorized)
        );
        env.set_fees(MAX_FEE_BPS, MAX_RESERVE_BPS).unwrap();
        let cfg = env.config();
        assert_eq!(
            (cfg.fee_bps, cfg.reserve_bps),
            (MAX_FEE_BPS, MAX_RESERVE_BPS)
        );

        let s = env.open_session(1);
        // A later change must not re-price the live session (L5).
        env.set_fees(0, 0).unwrap();
        let sess = env.session(&s);
        assert_eq!(
            (sess.fee_bps, sess.reserve_bps),
            (MAX_FEE_BPS, MAX_RESERVE_BPS)
        );
        assert_eq!(
            (sess.fee_receiver, sess.asset_mint),
            (env.fee, env.asset_mint)
        );

        let seller = env.party(BOND, UNIT);
        let buyer = env.party(200_000_000 + BOND, 0);
        env.deposit_ask(&s, &seller, UNIT).unwrap();
        env.deposit(&s, &buyer.key, &buyer.usdc, 200_000_000)
            .unwrap();
        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &seller.key.pubkey()).unwrap();
        env.mark_revealed(&s, &buyer.key.pubkey()).unwrap();
        env.set_time(REVEAL_END);
        let f = env.fill(&s, &buyer, &seller);
        env.settle(&f, 0, UNIT, PRICE).unwrap();
        let want = FillSplit::compute(UNIT, PRICE, 6, MAX_FEE_BPS, MAX_RESERVE_BPS).unwrap();
        assert_eq!(
            env.receipt(&s, 0).split,
            want,
            "{tp:?} snapshot fees applied"
        );
        assert_eq!(env.bal(&env.fee), want.fee);
    });
}

#[test]
fn session_asset_cannot_be_the_settlement_mint() {
    each(|tp| {
        let mut env = Env::new(tp);
        let op = env.authority.insecure_clone();
        let usdc = env.mint;
        let params = payment_rails::SessionParams {
            commit_end: COMMIT_END,
            reveal_end: REVEAL_END,
            settle_deadline: SETTLE_DEADLINE,
            bond_amount: BOND,
            max_cash: MAX_CASH,
            rofr_authority: Default::default(),
        };
        assert_eq!(
            env.open_session_full(&op, 1, params, usdc).unwrap_err(),
            err(RailsError::AssetMismatch),
            "{tp:?}"
        );
    });
}

/// AC-3 cash leg, many fills: a buyer filled against many sellers in odd
/// sizes; the crank refunds the excess; totals reconcile to the micro-USDC.
#[test]
fn many_small_fills_reconcile_exactly() {
    each(|tp| {
        let asks: Vec<u64> = (1..=6).map(|i| i * 333_333 + 7).collect();
        let Book {
            mut env,
            s,
            sellers,
            buyers,
        } = book(tp, &asks, &[1_000_000_000]);
        let x = &buyers[0];
        let price = 97_777_777;
        let mut paid = 0u64;
        for (i, (seller, &q)) in sellers.iter().zip(&asks).enumerate() {
            let f = env.fill(&s, x, seller);
            env.settle(&f, i as u32, q, price).unwrap();
            paid += split(q, price).gross;
        }
        assert_eq!(env.session(&s).gross_settled, paid);
        assert_eq!(env.escrow(&s, &x.key.pubkey()).cash, 1_000_000_000 - paid);
        env.finish_session(&s).unwrap();
        let crank = env.funded();
        env.refund(&crank, &s, &x.key.pubkey(), &x.usdc).unwrap();
        assert_eq!(
            env.bal(&x.usdc),
            1_000_000_000 + BOND - paid,
            "{tp:?} excess refunded exactly"
        );
        assert_eq!(env.bal(&x.asset), asks.iter().sum::<u64>());
        let seller_cash: u64 = sellers.iter().map(|p| env.bal(&p.usdc)).sum();
        assert_eq!(
            seller_cash + env.bal(&env.fee) + env.bal(&env.reserve),
            paid, // sellers' bonds are still in their escrows
            "{tp:?} every paid micro-USDC landed"
        );
    });
}
