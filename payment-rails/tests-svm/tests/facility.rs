//! liquidity-facility (#139, SRS 08 §4.5) against the real
//! `liquidity_facility.so`, `payment_rails.so`, `mark_engine.so`,
//! `auction_engine.so`, `credit_gate.so` and SAS: advances underwritten at
//! MARK_PROTECTIVE under the H1 ceiling and only on a Fresh mark; repay +
//! release; breach, grace and cure; **no liquidation on a Stale or
//! Suspended mark** (H4 / LF-P4); and every rung of the LF-P5 ladder —
//! issuer ROFR, backstop at its haircut (with a booked shortfall), and the
//! forced-sale auction with workout of the unsold remainder (AC-9).

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::{AnchorDeserialize, Discriminator, InstructionData};
use base64::Engine;
use common::*;
use credit_gate::pledge_holder;
use liquidity_facility::signer_pda as lf_signer;
use liquidity_facility::{
    FacilityParams, ForcedSaleSettled, ForcedSchedule, LadderSale, LfError, PositionState,
};
use mark_engine::MarkState;
use solana_keypair::Keypair;
use solana_signer::Signer;

const UNIT: u64 = 1_000_000;
const ID: u64 = 7;
const GRACE: i64 = 3_600;
const WINDOW: i64 = 600;

fn lf(e: LfError) -> Fail {
    Fail::Code(u32::from(e))
}

fn events<T: AnchorDeserialize + Discriminator>(logs: &[String]) -> Vec<T> {
    logs.iter()
        .filter_map(|l| l.strip_prefix("Program data: "))
        .filter_map(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .filter(|d| d.starts_with(T::DISCRIMINATOR))
        .map(|d| T::deserialize(&mut &d[T::DISCRIMINATOR.len()..]).unwrap())
        .collect()
}

struct Fx {
    env: Env,
    borrower: Party,
    issuer: Party,
    backstop: Party,
    lender: Keypair,
    crank: Keypair,
    cfg: Pubkey,
}

/// Facility (12% APR, 50% advance rate, 65% maintenance LTV, 1 h grace,
/// backstop haircut 15%) funded with 100 USDC; borrower holds 10 units
/// at a Fresh NAV of 1.00 and is a registered payout destination.
fn fx() -> Fx {
    let mut env = Env::new(Tp::Spl);
    let issuer_key = Keypair::new();
    let backstop_key = Keypair::new();
    let lender = Keypair::new();
    let params = FacilityParams {
        rate_bps: 1_200,
        advance_rate_bps: 5_000,
        maintenance_ltv_bps: 6_500,
        grace_seconds: GRACE,
        issuer: issuer_key.pubkey(),
        issuer_window: WINDOW,
        backstop: backstop_key.pubkey(),
        backstop_haircut_bps: 1_500,
        backstop_window: WINDOW,
        forced_sale_max_cash: 100 * UNIT,
        lender_workout: lender.pubkey(),
    };
    let _ = env.setup_facility(ID, params);
    // LF-P2: a facility drives the rails on the engine's one granted signer,
    // so only the upgrade authority opens one.
    let stranger = env.funded();
    let ix = env.open_facility_ix(ID + 1, params, &stranger.pubkey());
    assert_eq!(
        env.send(&[ix], &[&stranger]).unwrap_err(),
        lf(LfError::NotUpgradeAuthority)
    );
    let funder = env.party(100 * UNIT, 0);
    let rails_facility = lf_rails_facility(ID);
    env.fund_facility(&rails_facility, &funder, 100 * UNIT)
        .expect("fund");
    let borrower = env.party(0, 10 * UNIT);
    env.register(&borrower, T0 + 400 * 86_400)
        .expect("register");
    let issuer = with_key(&mut env, issuer_key, 20 * UNIT);
    let backstop = with_key(&mut env, backstop_key, 20 * UNIT);
    // The backstop buys gated units: the gate admits it (LF-P5). The
    // issuer buys its own security back and is not admitted.
    env.admit(&backstop.key.pubkey(), 0);
    let crank = env.funded();
    Fx {
        env,
        borrower,
        issuer,
        backstop,
        lender,
        crank,
        cfg: lf_config_pda(ID),
    }
}

/// A party around a known key (issuer / backstop are facility params).
fn with_key(env: &mut Env, key: Keypair, usdc: u64) -> Party {
    env.svm.airdrop(&key.pubkey(), 10_000_000_000).unwrap();
    let mint = env.mint;
    let usdc_acct = env.token_account(&mint, &key.pubkey(), usdc);
    let asset = env.asset_account(&key.pubkey(), 0);
    Party {
        key,
        usdc: usdc_acct,
        asset,
    }
}

/// Pledge all 10 units for 5 USDC, then a drill mark at `nav` breaches
/// it; past grace the ladder starts at the issuer.
fn breached(nav: u128) -> Fx {
    let mut f = fx();
    f.env.pledge(ID, &f.borrower, 10 * UNIT, 5 * UNIT).unwrap();
    let mint = f.env.asset_mint;
    let now = f.env.now();
    f.env.set_time(now + 60);
    f.env.drill_nav(&mint, nav).unwrap();
    let b = f.borrower.key.pubkey();
    f.env.check_health(ID, &b, &f.crank).unwrap();
    assert_eq!(f.env.position(&f.cfg, &b).state, PositionState::Breached);
    let now = f.env.now();
    f.env.set_time(now + GRACE);
    f.env.start_liquidation(ID, &b, &f.crank).unwrap();
    assert_eq!(f.env.position(&f.cfg, &b).state, PositionState::IssuerRofr);
    f
}

#[test]
fn advance_at_mark_protective_under_h1_then_repay_and_release() {
    let Fx {
        mut env,
        borrower,
        crank,
        cfg,
        ..
    } = fx();
    // 10 units × 1.00 × 50% = 5 USDC; a micro more is refused.
    assert_eq!(
        env.pledge(ID, &borrower, 10 * UNIT, 5 * UNIT + 1)
            .unwrap_err(),
        lf(LfError::AdvanceAboveRate)
    );
    let b = borrower.key.pubkey();
    let seasoned = env.lots(&b);
    env.pledge(ID, &borrower, 10 * UNIT, 5 * UNIT).unwrap();
    assert_eq!(env.bal(&borrower.usdc), 5 * UNIT);
    assert_eq!(env.bal(&borrower.asset), 0);
    // LF-P5: the lots went with the units, dates and all.
    let position = position_pda(&cfg, &b);
    let pledged = env.lots(&pledge_holder(&env.asset_mint, &lf_signer().0, &position));
    assert_eq!(env.lots(&b).total().unwrap(), 0, "borrower holds no lots");
    assert_eq!(pledged.total().unwrap(), 10 * UNIT);
    assert_eq!(pledged.lots[0].acquired_ts, seasoned.lots[0].acquired_ts);
    let pos = env.position(&cfg, &b);
    assert_eq!((pos.qty, pos.state), (10 * UNIT, PositionState::Active));
    // Healthy: no state change.
    env.check_health(ID, &b, &crank).unwrap();
    assert_eq!(env.position(&cfg, &b).state, PositionState::Active);
    // Not repaid yet: collateral stays.
    assert_eq!(
        env.release(ID, &borrower, &crank).unwrap_err(),
        lf(LfError::StillOwed)
    );
    // Repay everything (interest first) through the rails, then release.
    let now = env.now();
    env.set_time(now + 86_400);
    let payer = env.party(10 * UNIT, 0);
    env.repay(&lf_rails_facility(ID), &b, &payer, 10 * UNIT)
        .unwrap();
    env.release(ID, &borrower, &crank).unwrap();
    assert_eq!(env.bal(&borrower.asset), 10 * UNIT);
    assert_eq!(env.position(&cfg, &b).state, PositionState::Closed);
    // ...and came back with them, seasoning unchanged.
    let back = env.lots(&b);
    assert_eq!(back.total().unwrap(), 10 * UNIT);
    assert_eq!(back.sellable(env.now()).unwrap(), 10 * UNIT);
    assert_eq!(back.lots[0].acquired_ts, seasoned.lots[0].acquired_ts);
    assert_eq!(
        env.lots(&pledge_holder(&env.asset_mint, &lf_signer().0, &position))
            .total()
            .unwrap(),
        0
    );
}

/// LF-P5 / #202: pledged units take their seasoning with them. Before, the
/// borrower kept 10 seasoned lots for units sitting in the facility, and
/// could sell 10 freshly bought (unseasoned) units at once.
#[test]
fn lf_p5_a_pledge_takes_the_seasoning_with_the_units() {
    let Fx {
        mut env,
        borrower,
        cfg,
        ..
    } = fx();
    let b = borrower.key.pubkey();
    env.pledge(ID, &borrower, 10 * UNIT, 5 * UNIT).unwrap();
    // The borrower buys 10 more units now: unseasoned.
    let now = env.now();
    env.append_lot_at(&b, 10 * UNIT, now);
    let mine = env.lots(&b);
    assert_eq!(mine.total().unwrap(), 10 * UNIT);
    assert_eq!(
        mine.sellable(now).unwrap(),
        0,
        "nothing sellable for 7 days"
    );
    // The pledge ledger is the gate's, under a key nobody signs for: only
    // the facility (registered custodian) moves lots out of it.
    let position = position_pda(&cfg, &b);
    let holder = pledge_holder(&env.asset_mint, &lf_signer().0, &position);
    let stranger = env.funded();
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: credit_gate::ID,
        accounts: anchor_lang::ToAccountMetas::to_account_metas(
            &credit_gate::accounts::CustodyRelease {
                config: env.gate_config(),
                record: gate_pda(&[
                    credit_gate::Custodian::SEED,
                    env.asset_mint.as_ref(),
                    stranger.pubkey().as_ref(),
                ]),
                custodian: stranger.pubkey(),
                from_lots: env.lots_pda(&holder),
                to_lots: env.lots_pda(&b),
                to_block: env.block_entry(&b),
                mint: env.asset_mint,
            },
            None,
        ),
        data: credit_gate::instruction::CustodyRelease {
            pledge_ref: position,
            qty: 10 * UNIT,
        }
        .data(),
    };
    assert!(env.send(&[ix], &[&stranger]).is_err(), "not a custodian");
    assert_eq!(env.lots(&holder).total().unwrap(), 10 * UNIT);
}

#[test]
fn warned_or_stale_marks_block_new_advances() {
    let Fx {
        mut env, borrower, ..
    } = fx();
    env.set_time(T0 + 9 * 86_400); // NAV published at T0: Warned
    let mint = env.asset_mint;
    assert_eq!(env.feed(&mint).state_at(env.now()), MarkState::Warned);
    assert_eq!(
        env.pledge(ID, &borrower, 10 * UNIT, UNIT).unwrap_err(),
        lf(LfError::MarkNotFresh)
    );
}

#[test]
fn breach_waits_out_the_grace_and_a_top_up_cures_it() {
    let mut f = fx();
    f.env.pledge(ID, &f.borrower, 5 * UNIT, 2 * UNIT).unwrap();
    let b = f.borrower.key.pubkey();
    let mint = f.env.asset_mint;
    let now = f.env.now();
    f.env.set_time(now + 60);
    f.env.drill_nav(&mint, NAV_ONE / 2).unwrap(); // 5 × 0.50 = 2.5 → LTV 80%
    f.env.check_health(ID, &b, &f.crank).unwrap();
    assert_eq!(f.env.position(&f.cfg, &b).state, PositionState::Breached);
    assert_eq!(
        f.env.start_liquidation(ID, &b, &f.crank).unwrap_err(),
        lf(LfError::GraceActive)
    );
    // Top-up inside the grace window: 10 units × 0.50 = 5 → LTV 40%.
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: liquidity_facility::ID,
        accounts: anchor_lang::ToAccountMetas::to_account_metas(
            &liquidity_facility::accounts::AddCollateral {
                config: f.cfg,
                position: position_pda(&f.cfg, &b),
                signer: liquidity_facility::signer_pda().0,
                collateral_vault: collateral_pda(&position_pda(&f.cfg, &b)),
                borrower: b,
                borrower_asset: f.borrower.asset,
                asset_mint: mint,
                asset_token_program: f.env.asset_tp.id(),
                gate: f.env.lf_gate(&position_pda(&f.cfg, &b), &b),
            },
            None,
        ),
        data: liquidity_facility::instruction::AddCollateral { qty: 5 * UNIT }.data(),
    };
    f.env.send(&[ix], &[&f.borrower.key]).unwrap();
    f.env.check_health(ID, &b, &f.crank).unwrap();
    assert_eq!(f.env.position(&f.cfg, &b).state, PositionState::Active);
}

#[test]
fn no_liquidation_on_a_stale_or_suspended_mark() {
    let mut f = fx();
    f.env.pledge(ID, &f.borrower, 10 * UNIT, 5 * UNIT).unwrap();
    let b = f.borrower.key.pubkey();
    let mint = f.env.asset_mint;
    let now = f.env.now();
    f.env.set_time(now + 60);
    f.env.drill_nav(&mint, NAV_ONE * 6 / 10).unwrap();
    f.env.check_health(ID, &b, &f.crank).unwrap();
    // Suspended: the ladder may not start, whatever the grace.
    f.env
        .feed_admin(
            &mint,
            mark_engine::instruction::Suspend {
                reason_hash: [1; 32],
            }
            .data(),
        )
        .unwrap();
    let now = f.env.now();
    f.env.set_time(now + GRACE);
    assert_eq!(
        f.env.start_liquidation(ID, &b, &f.crank).unwrap_err(),
        lf(LfError::StaleMark)
    );
    f.env
        .feed_admin(&mint, mark_engine::instruction::Resume {}.data())
        .unwrap();
    // Stale by age (no mark for 15 days): still refused.
    let now = f.env.now();
    f.env.set_time(now + 15 * 86_400);
    assert_eq!(f.env.feed(&mint).state_at(f.env.now()), MarkState::Stale);
    assert_eq!(
        f.env.start_liquidation(ID, &b, &f.crank).unwrap_err(),
        lf(LfError::StaleMark)
    );
    // A fresh mark lets it run.
    f.env.drill_nav(&mint, NAV_ONE * 6 / 10).unwrap();
    f.env.start_liquidation(ID, &b, &f.crank).unwrap();
}

#[test]
fn ladder_step_one_issuer_rofr_at_mark_protective() {
    let Fx {
        mut env,
        borrower,
        issuer,
        backstop,
        cfg,
        ..
    } = breached(NAV_ONE * 7 / 10);
    // Only the issuer, only in its window.
    assert_eq!(
        env.ladder_buy(ID, &borrower, &backstop).unwrap_err(),
        lf(LfError::NotLadderBuyer)
    );
    let meta = env.ladder_buy(ID, &borrower, &issuer).unwrap();
    let sale = &events::<LadderSale>(&meta.logs)[0];
    // 10 units × 0.70 = 7 USDC; repays the 5 USDC advance + interest.
    assert_eq!(sale.price, 7 * UNIT);
    assert_eq!(sale.shortfall, 0);
    assert_eq!(sale.repaid + sale.to_borrower, 7 * UNIT);
    assert!(sale.repaid > 5 * UNIT, "interest accrued");
    assert_eq!(env.bal(&issuer.asset), 10 * UNIT);
    assert_eq!(env.bal(&borrower.usdc), 5 * UNIT + sale.to_borrower);
    let b = borrower.key.pubkey();
    assert_eq!(env.position(&cfg, &b).state, PositionState::Closed);
    // The rails advance is closed (fully repaid).
    assert!(!env.exists(&advance_pda(&lf_rails_facility(ID), &b)));
}

#[test]
fn ladder_step_two_backstop_at_its_haircut_books_the_shortfall() {
    let Fx {
        mut env,
        borrower,
        issuer,
        backstop,
        crank,
        cfg,
        ..
    } = breached(NAV_ONE / 2);
    let b = borrower.key.pubkey();
    // Someone else cannot pass the issuer's step early.
    assert_eq!(
        env.pass_step(ID, &b, &crank).unwrap_err(),
        lf(LfError::NotLadderBuyer)
    );
    env.pass_step(ID, &b, &issuer.key).unwrap(); // issuer declines
    assert_eq!(env.position(&cfg, &b).state, PositionState::Backstop);
    let meta = env.ladder_buy(ID, &borrower, &backstop).unwrap();
    let sale = &events::<LadderSale>(&meta.logs)[0];
    // 10 × 0.50 = 5 USDC × (1 − 15%) = 4.25 < debt → shortfall.
    assert_eq!(sale.price, 4_250_000);
    assert_eq!(sale.repaid, 4_250_000);
    assert_eq!(sale.to_borrower, 0);
    assert!(sale.shortfall > 750_000, "debt 5 USDC + interest − 4.25");
    let pos = env.position(&cfg, &b);
    assert_eq!(
        (pos.state, pos.shortfall),
        (PositionState::Closed, sale.shortfall)
    );
    assert_eq!(env.bal(&backstop.asset), 10 * UNIT);
    // LF-P5: a purchase — the backstop's lot seasons from now; the pledge
    // ledger's seasoned lots were debited.
    let bought = env.lots(&backstop.key.pubkey());
    assert_eq!(bought.total().unwrap(), 10 * UNIT);
    assert_eq!(bought.sellable(env.now()).unwrap(), 0);
    assert_eq!(bought.lots[0].source, credit_gate::LotSource::CustodySale);
    let position = position_pda(&cfg, &b);
    assert_eq!(
        env.lots(&pledge_holder(&env.asset_mint, &lf_signer().0, &position))
            .total()
            .unwrap(),
        0
    );
}

#[test]
fn ladder_step_three_forced_sale_then_workout_of_the_unsold_rest() {
    let Fx {
        mut env,
        borrower,
        issuer,
        lender,
        crank,
        cfg,
        ..
    } = breached(NAV_ONE * 6 / 10);
    let b = borrower.key.pubkey();
    env.pass_step(ID, &b, &issuer.key).unwrap();
    // Backstop silent: anyone passes once its window closes.
    assert_eq!(
        env.pass_step(ID, &b, &crank).unwrap_err(),
        lf(LfError::NotLadderBuyer)
    );
    let now = env.now();
    env.set_time(now + WINDOW);
    env.pass_step(ID, &b, &crank).unwrap();
    assert_eq!(env.position(&cfg, &b).state, PositionState::ForcedSale);

    // The facility's signer sells. Its lots are the borrower's, moved by
    // the pledge (LF-P5): no gate key appends any. (It is admitted, as the
    // holder of record of the vault, but holds no lots of its own.)
    let signer = liquidity_facility::signer_pda().0;
    env.admit(&signer, 0);
    let t = env.now();
    let s = ForcedSchedule {
        commit_end: t + 100,
        reveal_end: t + 200,
        settle_deadline: t + 400,
    };
    let lister = env.party(UNIT, 0);
    let crank_k = crank.insecure_clone();
    env.open_forced_sale(ID, &b, &crank_k, 9, s).unwrap();
    env.list_forced_sale(ID, &b, &lister, 9).unwrap();
    // One bidder wants 4 units ≤ 0.55.
    let bidder = env.party(10 * UNIT + BOND, 0);
    let bid = Sealed {
        side: auction_engine::Side::Buy,
        limit: 550_000,
        qty: 4 * UNIT,
        nonce: [5; 32],
    };
    env.commit_bid(9, &bidder, &bid, 5 * UNIT).unwrap();
    env.set_time(s.commit_end);
    env.reveal_forced_sale(ID, &b, &crank, 9).unwrap();
    env.reveal(9, &bidder, &bid).unwrap();
    env.set_time(s.reveal_end);
    env.clear_auction(9, &crank).unwrap();
    let a = env.auction(9);
    assert_eq!((a.clear_price, a.cleared_volume), (550_000, 4 * UNIT));
    env.allocate(9, &crank, 4).unwrap();
    let f = env.forced_fill(ID, &b, 9, &bidder);
    env.settle_auction_fill(9, &crank, 0, f).unwrap();
    let meta = env.finish_auction(9, &crank).unwrap();
    let print = &events::<auction_engine::ClearingPrint>(&meta.logs)[0];
    assert!(print.forced_sale, "flagged: excluded from marks (LF-2)");
    assert_eq!(env.feed(&env.asset_mint).prints_accepted, 0);

    // Not before the ask escrow is refunded.
    let workout = env.asset_account(&lender.pubkey(), 0);
    assert_eq!(
        env.settle_forced_sale(ID, &borrower, &crank, 9, &workout)
            .unwrap_err(),
        lf(LfError::SaleNotDone)
    );
    env.refund_forced_ask(ID, &b, &crank, 9).unwrap();
    let meta = env
        .settle_forced_sale(ID, &borrower, &crank, 9, &workout)
        .unwrap();
    let r = &events::<ForcedSaleSettled>(&meta.logs)[0];
    assert_eq!(
        (r.sold, r.price, r.to_workout),
        (4 * UNIT, 550_000, 6 * UNIT)
    );
    assert_eq!(r.to_borrower, 0);
    assert!(
        r.shortfall > 0,
        "2.2 USDC of proceeds against a 5 USDC debt"
    );
    assert_eq!(env.bal(&workout), 6 * UNIT);
    assert_eq!(env.bal(&bidder.asset), 4 * UNIT);
    assert_eq!(env.position(&cfg, &b).state, PositionState::Workout);
    // LF-P5: settle_check debited the 4 sold from the signer's ledger
    // (moved there from the pledge at listing); the lender holds the 6
    // unsold with their original, seasoned dates; nothing is left behind.
    let lender_lots = env.lots(&lender.pubkey());
    assert_eq!(lender_lots.total().unwrap(), 6 * UNIT);
    assert_eq!(lender_lots.sellable(env.now()).unwrap(), 6 * UNIT);
    assert_eq!(env.lots(&signer).total().unwrap(), 0);
    let position = position_pda(&cfg, &b);
    assert_eq!(
        env.lots(&pledge_holder(&env.asset_mint, &signer, &position))
            .total()
            .unwrap(),
        0
    );
}

/// LF-P5: collateral still seasoning cannot be force-sold (C2). The listing
/// refuses it, the session fails, and the lender takes the units with
/// their dates on the workout.
#[test]
fn lf_p5_unseasoned_collateral_goes_to_workout_not_to_a_forced_sale() {
    let mut f = fx();
    // A second borrower whose 10 units were acquired just now.
    let (k, usdc) = f.env.bidder(0);
    let asset = f.env.asset_account(&k.pubkey(), 10 * UNIT);
    f.env.admit(&k.pubkey(), 0);
    let now = f.env.now();
    f.env.append_lot_at(&k.pubkey(), 10 * UNIT, now);
    let fresh = Party {
        key: k,
        usdc,
        asset,
    };
    f.env.register(&fresh, T0 + 400 * 86_400).expect("register");
    f.env.pledge(ID, &fresh, 10 * UNIT, 5 * UNIT).unwrap();
    let b = fresh.key.pubkey();
    let mint = f.env.asset_mint;
    let t = f.env.now();
    f.env.set_time(t + 60);
    f.env.drill_nav(&mint, NAV_ONE * 6 / 10).unwrap();
    f.env.check_health(ID, &b, &f.crank).unwrap();
    let t = f.env.now();
    f.env.set_time(t + GRACE);
    f.env.start_liquidation(ID, &b, &f.crank).unwrap();
    f.env.pass_step(ID, &b, &f.issuer.key).unwrap();
    // The backstop cannot buy unseasoned units either.
    assert!(f.env.ladder_buy(ID, &fresh, &f.backstop).is_err());
    f.env.pass_step(ID, &b, &f.backstop.key).unwrap();
    let signer = lf_signer().0;
    f.env.admit(&signer, 0);
    let t = f.env.now();
    let s = ForcedSchedule {
        commit_end: t + 100,
        reveal_end: t + 200,
        settle_deadline: t + 400,
    };
    let crank = f.crank.insecure_clone();
    f.env.open_forced_sale(ID, &b, &crank, 9, s).unwrap();
    let lister = f.env.party(UNIT, 0);
    assert_eq!(
        f.env.list_forced_sale(ID, &b, &lister, 9).unwrap_err(),
        lf(LfError::NothingSeasoned)
    );
    f.env.set_time(s.reveal_end);
    f.env.clear_auction(9, &crank).unwrap(); // no orders: fails
    assert_eq!(f.env.auction(9).phase, auction_engine::Phase::Failed);
    let workout = f.env.asset_account(&f.lender.pubkey(), 0);
    let meta = f
        .env
        .settle_forced_sale(ID, &fresh, &crank, 9, &workout)
        .unwrap();
    let r = &events::<ForcedSaleSettled>(&meta.logs)[0];
    assert_eq!((r.sold, r.to_workout), (0, 10 * UNIT));
    let lender_lots = f.env.lots(&f.lender.pubkey());
    assert_eq!(lender_lots.total().unwrap(), 10 * UNIT);
    assert_eq!(lender_lots.lots[0].acquired_ts, now, "tacked: same date");
    assert_eq!(
        lender_lots.sellable(f.env.now()).unwrap(),
        0,
        "still seasoning"
    );
}
