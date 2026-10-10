//! auction-engine (#136, SRS 08 §4.2) end to end against the real
//! `auction_engine.so`, `payment_rails.so`, `credit_gate.so`, SAS and
//! `mark_engine.so`: three bidders and a seller commit sealed orders with
//! rails escrow, reveal, clear at one uniform price at a discount to NAV
//! (AC-3/AC-4), split the margin pro-rata, the issuer takes part of a fill
//! by ROFR (AC-5), every fill settles DvP through the rails with the gate's
//! `settle_check`, the print lands in mark-engine, and every unfilled
//! micro-USDC is refundable (AC-11). Failure paths: too few reveals, no
//! cross above the reserve, a suspended mark, a bad preimage, the deadline.
//! AE-P6 / AC-7: the gate check excludes a would-be investor past the holder
//! cap, a block-listed buyer and an unseasoned seller before the clear, and
//! their volume is reallocated to eligible orders.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::{AnchorDeserialize, Discriminator, InstructionData};
use auction_engine::{
    instruction as aix, AuctionError, ClearingPrint, Exclusion, FailReason, OrderScreened, Phase,
    SessionFailed, SessionParams, Side,
};
use base64::Engine;
use common::*;
use mark_engine::MarkState;
use payment_rails::{RailsError, SessionStatus};
use solana_keypair::Keypair;
use solana_signer::Signer;

const UNIT: u64 = 1_000_000; // one whole asset unit (6 dp)
const COMMIT_END_A: i64 = T0 + 100;
const REVEAL_END_A: i64 = T0 + 200;
const DEADLINE_A: i64 = T0 + 400;

fn aerr(e: AuctionError) -> Fail {
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

fn params(rofr: Pubkey) -> SessionParams {
    SessionParams {
        commit_end: COMMIT_END_A,
        reveal_end: REVEAL_END_A,
        rofr_window: if rofr == Pubkey::default() { 0 } else { 50 },
        settle_deadline: DEADLINE_A,
        bond: BOND,
        max_cash: MAX_CASH,
        min_reveals: 3,
        max_discount_bps: 1_000, // reserve = 0.90 at a Fresh NAV of 1.00
        forced_sale: false,
        rofr_authority: rofr,
        disclosure_hash: [0xd1; 32],
    }
}

fn sealed(side: Side, limit: u64, qty: u64, n: u8) -> Sealed {
    Sealed {
        side,
        limit,
        qty,
        nonce: [n; 32],
    }
}

/// Sum of every USDC account in play (conservation).
fn usdc_total(env: &Env, accounts: &[Pubkey]) -> u64 {
    accounts.iter().map(|a| env.bal(a)).sum()
}

struct Book {
    env: Env,
    seller: Party,
    b1: Party,
    b2: Party,
    b3: Party,
    issuer: Party,
    crank: Keypair,
    orders: [Sealed; 4],
}

/// Seller asks 10 @ 0.90; B1 bids 6 @ 0.97, B2 6 @ 0.95, B3 4 @ 0.93.
/// Uniform price 0.95 (max volume 10, least imbalance); B1 and B2 split
/// the 10 pro-rata (5 + 5); B3 is out of the money.
fn book(tp: Tp, rofr: bool) -> Book {
    let mut env = Env::new(tp);
    env.setup_auction();
    let seller = env.party(BOND, 10 * UNIT);
    let b1 = env.party(10 * UNIT + BOND, 0);
    let b2 = env.party(10 * UNIT + BOND, 0);
    let b3 = env.party(10 * UNIT + BOND, 0);
    let issuer = env.party(10 * UNIT, 0);
    let crank = env.funded();
    let rofr_key = if rofr {
        issuer.key.pubkey()
    } else {
        Pubkey::default()
    };
    env.open_auction(1, params(rofr_key)).expect("open");
    let orders = [
        sealed(Side::Sell, 900_000, 10 * UNIT, 1),
        sealed(Side::Buy, 970_000, 6 * UNIT, 2),
        sealed(Side::Buy, 950_000, 6 * UNIT, 3),
        sealed(Side::Buy, 930_000, 4 * UNIT, 4),
    ];
    env.commit_ask(1, &seller, &orders[0], 10 * UNIT)
        .expect("ask");
    env.commit_bid(1, &b1, &orders[1], 10 * UNIT).expect("b1");
    env.commit_bid(1, &b2, &orders[2], 10 * UNIT).expect("b2");
    env.commit_bid(1, &b3, &orders[3], 10 * UNIT).expect("b3");
    Book {
        env,
        seller,
        b1,
        b2,
        b3,
        issuer,
        crank,
        orders,
    }
}

fn reveal_all(bk: &mut Book) {
    bk.env.set_time(COMMIT_END_A);
    let Book {
        env,
        seller,
        b1,
        b2,
        b3,
        orders,
        ..
    } = bk;
    env.reveal(1, seller, &orders[0]).expect("reveal s");
    env.reveal(1, b1, &orders[1]).expect("reveal b1");
    env.reveal(1, b2, &orders[2]).expect("reveal b2");
    env.reveal(1, b3, &orders[3]).expect("reveal b3");
}

#[test]
fn three_bidders_clear_at_a_discount_settle_dvp_and_print() {
    for tp in Tp::ALL {
        let mut bk = book(tp, true);
        let all_usdc = [
            bk.seller.usdc,
            bk.b1.usdc,
            bk.b2.usdc,
            bk.b3.usdc,
            bk.issuer.usdc,
            bk.env.fee,
            bk.env.reserve,
        ];
        let rs = bk.env.rails_session_of(1);
        let vaults: Vec<Pubkey> = [&bk.seller, &bk.b1, &bk.b2, &bk.b3]
            .iter()
            .map(|p| vault_pda(&escrow_pda(&rs, &p.key.pubkey())))
            .collect();
        let before = usdc_total(&bk.env, &all_usdc) + usdc_total(&bk.env, &vaults);
        reveal_all(&mut bk);
        let Book {
            mut env,
            seller,
            b1,
            b2,
            b3,
            issuer,
            crank,
            ..
        } = bk;

        // Clearing is permissionless, after reveals close.
        assert_eq!(
            env.clear_auction(1, &crank).unwrap_err(),
            aerr(AuctionError::OutsideWindow)
        );
        env.set_time(REVEAL_END_A);
        env.clear_auction(1, &crank).expect("clear");
        let s = env.auction(1);
        assert_eq!(s.phase, Phase::Allocating);
        assert_eq!(s.clear_price, 950_000, "uniform price, 5% under NAV (AC-4)");
        assert_eq!(s.reserve_price, 900_000);
        assert_eq!(s.cleared_volume, 10 * UNIT);
        let alloc: Vec<u64> = s.orders.iter().map(|o| o.alloc).collect();
        assert_eq!(
            alloc,
            vec![10 * UNIT, 5 * UNIT, 5 * UNIT, 0],
            "pro-rata margin"
        );

        // Allocation is chunked and resumable.
        env.allocate(1, &crank, 1).unwrap();
        assert_eq!(env.auction(1).phase, Phase::Allocating);
        env.allocate(1, &crank, 8).unwrap();
        let s = env.auction(1);
        assert_eq!(s.fills.len(), 2);
        assert_eq!(s.phase, Phase::IssuerRofr);
        assert_eq!(
            (s.fills[0].buyer, s.fills[0].qty),
            (b1.key.pubkey(), 5 * UNIT)
        );

        // AC-5: the issuer takes 2 of fill 0 at the clearing price.
        let outsider = env.party(10 * UNIT, 0);
        assert_eq!(
            env.exercise_rofr(1, &outsider, &seller, 0, 2 * UNIT)
                .unwrap_err(),
            aerr(AuctionError::NotRofrAuthority)
        );
        env.exercise_rofr(1, &issuer, &seller, 0, 2 * UNIT)
            .expect("rofr");
        assert_eq!(env.bal(&issuer.asset), 2 * UNIT);
        // Settling cannot start before the ROFR window closes.
        assert_eq!(
            env.settle_auction(1, &crank, 1, &b2, &seller).unwrap_err(),
            aerr(AuctionError::WrongPhase)
        );
        env.close_rofr(1, &issuer.key).unwrap();

        // AE-P8: one fill per instruction, permissionless, through the rails.
        env.settle_auction(1, &crank, 0, &b1, &seller)
            .expect("fill 0");
        // Replays fail: the engine marks it settled, the rails receipt exists.
        assert_eq!(
            env.settle_auction(1, &crank, 0, &b1, &seller).unwrap_err(),
            aerr(AuctionError::FillSettled)
        );
        // A crank cannot point a fill at a different buyer's escrow.
        assert_eq!(
            env.settle_auction(1, &crank, 1, &b3, &seller).unwrap_err(),
            aerr(AuctionError::RailsAccounts)
        );
        assert_eq!(
            env.finish_auction(1, &crank).unwrap_err(),
            aerr(AuctionError::FillsPending)
        );
        env.settle_auction(1, &crank, 1, &b2, &seller)
            .expect("fill 1");
        assert_eq!(env.bal(&b1.asset), 3 * UNIT);
        assert_eq!(env.bal(&b2.asset), 5 * UNIT);
        assert_eq!(env.bal(&b3.asset), 0);

        // AE-P9: finish → rails Settled + print into mark-engine.
        let meta = env.finish_auction(1, &crank).expect("finish");
        let print = &events::<ClearingPrint>(&meta.logs)[0];
        assert_eq!(
            (print.price, print.volume, print.n_investor_ids),
            (950_000, 10 * UNIT, 4)
        );
        assert!(!print.forced_sale);
        assert_eq!(env.auction(1).phase, Phase::Printed);
        assert_eq!(env.session(&rs).status, SessionStatus::Settled);
        let feed = env.feed(&env.asset_mint);
        assert_eq!(feed.prints_accepted, 1);
        assert_eq!(
            feed.mark_protective(env.now()).unwrap(),
            NAV_ONE * 95 / 100,
            "the print set the implied curve"
        );

        // AC-11: buyers' unused cash and every bond come back, by anyone.
        for p in [&b1, &b2, &b3] {
            env.refund(&crank, &rs, &p.key.pubkey(), &p.usdc)
                .expect("refund");
        }
        env.refund_ask(&crank, &rs, &seller).expect("refund ask");
        assert_eq!(env.bal(&b3.usdc), 10 * UNIT + BOND, "B3 whole again");
        let gross = |q: u64| q * 950_000 / UNIT;
        assert_eq!(env.bal(&b1.usdc), 10 * UNIT + BOND - gross(3 * UNIT));
        assert_eq!(env.bal(&b2.usdc), 10 * UNIT + BOND - gross(5 * UNIT));
        assert_eq!(env.bal(&issuer.usdc), 10 * UNIT - gross(2 * UNIT));
        let fees = env.bal(&env.fee) + env.bal(&env.reserve);
        assert_eq!(env.bal(&seller.usdc), BOND + gross(10 * UNIT) - fees);
        // Conservation: every micro-USDC accounted for.
        assert_eq!(usdc_total(&env, &all_usdc), before);
    }
}

#[test]
fn too_few_reveals_fails_the_session_and_everyone_refunds() {
    let mut bk = book(Tp::Spl, false);
    bk.env.set_time(COMMIT_END_A);
    {
        let Book {
            env,
            seller,
            b1,
            orders,
            ..
        } = &mut bk;
        env.reveal(1, seller, &orders[0]).unwrap();
        env.reveal(1, b1, &orders[1]).unwrap();
    }
    let Book {
        mut env,
        seller,
        b1,
        b2,
        crank,
        ..
    } = bk;
    env.set_time(REVEAL_END_A);
    let meta = env.clear_auction(1, &crank).expect("clear fails cleanly");
    assert_eq!(
        events::<SessionFailed>(&meta.logs)[0].reason,
        FailReason::TooFewReveals
    );
    assert_eq!(env.auction(1).phase, Phase::Failed);
    let rs = env.rails_session_of(1);
    assert_eq!(env.session(&rs).status, SessionStatus::Failed);
    // Revealed escrows refund whole; the unrevealed bidder loses the bond
    // (reveals had closed) — AE-P4.
    env.refund(&crank, &rs, &b1.key.pubkey(), &b1.usdc).unwrap();
    assert_eq!(env.bal(&b1.usdc), 10 * UNIT + BOND);
    env.refund(&crank, &rs, &b2.key.pubkey(), &b2.usdc).unwrap();
    assert_eq!(env.bal(&b2.usdc), 10 * UNIT);
    env.refund_ask(&crank, &rs, &seller).unwrap();
    assert_eq!(env.bal(&seller.asset), 10 * UNIT);
}

#[test]
fn no_cross_at_or_above_the_reserve_fails() {
    let mut env = Env::new(Tp::Spl);
    env.setup_auction();
    let seller = env.party(BOND, 10 * UNIT);
    let bs: Vec<Party> = (0..3).map(|_| env.party(10 * UNIT + BOND, 0)).collect();
    env.open_auction(1, params(Pubkey::default())).unwrap();
    // Everyone would trade at 0.85 — below the 0.90 reserve (A11).
    let ask = sealed(Side::Sell, 800_000, 10 * UNIT, 1);
    env.commit_ask(1, &seller, &ask, 10 * UNIT).unwrap();
    let bids: Vec<Sealed> = (0..3)
        .map(|i| sealed(Side::Buy, 850_000, 2 * UNIT, 10 + i as u8))
        .collect();
    for (b, o) in bs.iter().zip(&bids) {
        env.commit_bid(1, b, o, 5 * UNIT).unwrap();
    }
    env.set_time(COMMIT_END_A);
    env.reveal(1, &seller, &ask).unwrap();
    for (b, o) in bs.iter().zip(&bids) {
        env.reveal(1, b, o).unwrap();
    }
    env.set_time(REVEAL_END_A);
    let crank = env.funded();
    let meta = env.clear_auction(1, &crank).unwrap();
    assert_eq!(
        events::<SessionFailed>(&meta.logs)[0].reason,
        FailReason::NoCross
    );
}

#[test]
fn a_suspended_mark_blocks_clearing() {
    let mut bk = book(Tp::Spl, false);
    reveal_all(&mut bk);
    let mint = bk.env.asset_mint;
    bk.env
        .feed_admin(
            &mint,
            mark_engine::instruction::Suspend {
                reason_hash: [9; 32],
            }
            .data(),
        )
        .unwrap();
    assert_eq!(
        bk.env.feed(&mint).state_at(bk.env.now()),
        MarkState::Suspended
    );
    bk.env.set_time(REVEAL_END_A);
    let crank = bk.crank.insecure_clone();
    let meta = bk.env.clear_auction(1, &crank).unwrap();
    assert_eq!(
        events::<SessionFailed>(&meta.logs)[0].reason,
        FailReason::MarkUnavailable
    );
}

#[test]
fn reveals_must_match_the_commitment_and_the_escrow() {
    let mut bk = book(Tp::Spl, false);
    bk.env.set_time(COMMIT_END_A);
    let Book {
        mut env, b1, b3, ..
    } = bk;
    // Wrong nonce.
    assert_eq!(
        env.reveal(1, &b1, &sealed(Side::Buy, 970_000, 6 * UNIT, 9))
            .unwrap_err(),
        aerr(AuctionError::CommitmentMismatch)
    );
    // Commitment honoured, but 20 @ 0.93 costs more than the 10 escrowed.
    let mut env2 = Env::new(Tp::Spl);
    env2.setup_auction();
    let p = env2.party(30 * UNIT, 0);
    env2.open_auction(2, params(Pubkey::default())).unwrap();
    let greedy = sealed(Side::Buy, 930_000, 20 * UNIT, 5);
    env2.commit_bid(2, &p, &greedy, 10 * UNIT).unwrap();
    env2.set_time(COMMIT_END_A);
    assert_eq!(
        env2.reveal(2, &p, &greedy).unwrap_err(),
        aerr(AuctionError::BidExceedsEscrow)
    );
    // Revealing after the window closes is refused.
    env.set_time(REVEAL_END_A);
    assert_eq!(
        env.reveal(1, &b3, &sealed(Side::Buy, 930_000, 4 * UNIT, 4))
            .unwrap_err(),
        aerr(AuctionError::OutsideWindow)
    );
}

#[test]
fn only_the_venue_opens_and_only_the_opener_forces_a_sale() {
    let mut env = Env::new(Tp::Spl);
    env.setup_auction();
    let stranger = env.funded();
    assert_eq!(
        env.open_auction_as(&stranger, 1, params(Pubkey::default()))
            .unwrap_err(),
        aerr(AuctionError::NotVenueAuthority)
    );
    let mut forced = params(Pubkey::default());
    forced.forced_sale = true;
    assert_eq!(
        env.open_auction(1, forced).unwrap_err(),
        aerr(AuctionError::NotForcedSaleOpener)
    );
    // Without the rails grant the operator PDA cannot open a cash session.
    let mut env = Env::new(Tp::Spl);
    env.setup_auction();
    let revoke = anchor_lang::solana_program::instruction::Instruction {
        program_id: payment_rails::ID,
        accounts: anchor_lang::ToAccountMetas::to_account_metas(
            &payment_rails::accounts::SetEngineRoles {
                config: env.config,
                grant: grant_pda(&auction_engine::operator_pda().0),
                authority: env.authority.pubkey(),
            },
            None,
        ),
        data: payment_rails::instruction::SetEngineRoles { roles: 0 }.data(),
    };
    let a = env.authority.insecure_clone();
    env.send(&[revoke], &[&a]).unwrap();
    assert_eq!(
        env.open_auction(1, params(Pubkey::default())).unwrap_err(),
        Fail::rails(RailsError::EngineNotGranted)
    );
}

#[test]
fn past_the_deadline_any_session_expires_and_refunds() {
    let mut bk = book(Tp::Spl, false);
    reveal_all(&mut bk);
    let Book {
        mut env, b1, crank, ..
    } = bk;
    env.set_time(REVEAL_END_A);
    env.clear_auction(1, &crank).unwrap();
    // The crank dies before allocating; at the deadline anyone expires it.
    assert_eq!(
        env.expire_auction(1, &crank).unwrap_err(),
        aerr(AuctionError::OutsideWindow)
    );
    env.set_time(DEADLINE_A);
    let meta = env.expire_auction(1, &crank).unwrap();
    assert_eq!(
        events::<SessionFailed>(&meta.logs)[0].reason,
        FailReason::Deadline
    );
    let rs = env.rails_session_of(1);
    env.refund(&crank, &rs, &b1.key.pubkey(), &b1.usdc).unwrap();
    assert_eq!(env.bal(&b1.usdc), 10 * UNIT + BOND);
}

/// The venue's authority opens every session: only the upgrade authority may create it.
#[test]
fn only_the_upgrade_authority_creates_the_venue() {
    let mut env = Env::new(Tp::Spl);
    load_root_program(&mut env, auction_engine::ID, "auction_engine");
    let stranger = env.funded();
    let ix = venue_init_ix(&stranger.pubkey());
    assert_eq!(
        env.send(&[ix], &[&stranger]).unwrap_err(),
        aerr(AuctionError::NotUpgradeAuthority)
    );
    let auth = env.authority.insecure_clone();
    env.send(&[venue_init_ix(&auth.pubkey())], &[&auth])
        .expect("the upgrade authority creates it");
}

fn screen_reasons(env: &mut Env, id: u64, n: u8, crank: &Keypair) -> Vec<Option<Exclusion>> {
    (0..n)
        .map(|i| {
            let m = env.screen(id, i, crank).expect("screen");
            events::<OrderScreened>(&m.logs)[0].exclusion
        })
        .collect()
}

/// AE-P6 / AC-7: every revealed order passes the gate check before the
/// clear. A would-be new investor past the holder cap (the gate refused to
/// admit it) holds the best bid, and an admitted buyer is block-listed
/// after revealing; both are excluded with their reason, the clear runs
/// over the eligible orders, so the volume the best bid would have taken is
/// reallocated to admitted holders. Every fill settles (no ineligible fill
/// can wedge the session) and the excluded bidders get cash and bond back.
#[test]
fn ae_p6_ineligible_bids_are_excluded_and_their_volume_reallocated() {
    for tp in Tp::ALL {
        let mut env = Env::new(tp);
        env.setup_auction();
        let seller = env.party(BOND, 10 * UNIT);
        let b2 = env.party(10 * UNIT + BOND, 0);
        let b3 = env.party(10 * UNIT + BOND, 0);
        let blocked = env.party(10 * UNIT + BOND, 0);
        let crank = env.funded();

        // CG-P4: the cap is full, so X attests but `validate` refuses it.
        let full = env.policy_state().distinct_investors;
        env.set_holder_cap(full);
        let x = env.stranger(10 * UNIT + BOND);
        let xk = x.key.pubkey();
        let (id_schema, acc_schema) = (env.sas.id_schema, env.sas.acc_schema);
        env.sas_attest(
            id_schema,
            &xk,
            identity_data(investor_of(&xk)),
            ATTESTATION_EXPIRY,
        );
        env.sas_attest(acc_schema, &xk, vec![1, 1], ATTESTATION_EXPIRY);
        env.validate(&xk).expect("decision recorded");
        assert!(!env.exists(&env.link_pda(&xk)), "GovernorCap: not admitted");
        assert_eq!(env.policy_state().distinct_investors, full);

        env.open_auction(1, params(Pubkey::default())).unwrap();
        let orders = [
            sealed(Side::Sell, 900_000, 10 * UNIT, 1),
            sealed(Side::Buy, 990_000, 10 * UNIT, 2), // X: the best bid
            sealed(Side::Buy, 970_000, 10 * UNIT, 3), // blocked after reveal
            sealed(Side::Buy, 950_000, 6 * UNIT, 4),  // B2
            sealed(Side::Buy, 930_000, 6 * UNIT, 5),  // B3
        ];
        let bidders = [&x, &blocked, &b2, &b3];
        env.commit_ask(1, &seller, &orders[0], 10 * UNIT).unwrap();
        for (p, o) in bidders.iter().zip(&orders[1..]) {
            env.commit_bid(1, p, o, 10 * UNIT).expect("commit");
        }
        env.set_time(COMMIT_END_A);
        env.reveal(1, &seller, &orders[0]).unwrap();
        for (p, o) in bidders.iter().zip(&orders[1..]) {
            env.reveal(1, p, o).expect("reveal");
        }
        env.block(&blocked.key.pubkey());

        // The check runs once reveals close, and before any clear.
        assert_eq!(
            env.screen(1, 1, &crank).unwrap_err(),
            aerr(AuctionError::OutsideWindow)
        );
        env.set_time(REVEAL_END_A);
        assert_eq!(
            env.clear_only(1, &crank).unwrap_err(),
            aerr(AuctionError::ScreeningPending)
        );
        // A crank cannot swap in another wallet's attestation to exclude
        // B2, nor screen a bid as an ask.
        let mut bad = env.screen_ix(1, 3);
        bad.accounts[7].pubkey = env.identity_att(&b3.key.pubkey());
        assert_eq!(
            env.send(&[bad], &[&crank]).unwrap_err(),
            aerr(AuctionError::GateAccounts)
        );
        let mut bad = env.screen_ix(1, 0);
        bad.data = aix::ScreenAsk { order_index: 1 }.data();
        assert_eq!(
            env.send(&[bad], &[&crank]).unwrap_err(),
            aerr(AuctionError::WrongSide)
        );

        let reasons = screen_reasons(&mut env, 1, 5, &crank);
        assert_eq!(
            reasons,
            vec![
                None,
                Some(Exclusion::NotAdmitted),
                Some(Exclusion::Blocklisted),
                None,
                None
            ]
        );
        assert_eq!(env.auction(1).phase, Phase::Clearing);
        assert_eq!(
            env.screen(1, 3, &crank).unwrap_err(),
            aerr(AuctionError::AlreadyScreened)
        );

        // The clear sees only the eligible book: B2 and B3 split the 10.
        env.clear_only(1, &crank).expect("clear");
        let s = env.auction(1);
        assert_eq!(s.phase, Phase::Allocating);
        assert_eq!(s.cleared_volume, 10 * UNIT);
        assert!(s.clear_price <= 930_000, "X's 0.99 did not set the price");
        let alloc: Vec<u64> = s.orders.iter().map(|o| o.alloc).collect();
        assert_eq!(alloc, vec![10 * UNIT, 0, 0, 5 * UNIT, 5 * UNIT], "AC-7");

        env.allocate(1, &crank, 8).unwrap();
        env.settle_auction(1, &crank, 0, &b2, &seller)
            .expect("fill 0");
        env.settle_auction(1, &crank, 1, &b3, &seller)
            .expect("fill 1");
        env.finish_auction(1, &crank).expect("finish");
        assert_eq!(env.bal(&b2.asset), 5 * UNIT);
        assert_eq!(env.bal(&b3.asset), 5 * UNIT);
        assert_eq!(env.bal(&x.asset), 0);
        let rs = env.rails_session_of(1);
        for p in [&x, &blocked] {
            env.refund(&crank, &rs, &p.key.pubkey(), &p.usdc)
                .expect("refund");
            assert_eq!(env.bal(&p.usdc), 10 * UNIT + BOND, "excluded: whole");
        }
    }
}

/// AE-P6, sell side (C2): an ask whose seasoned lots do not cover it is
/// excluded, so a cheaper unseasoned ask cannot take the fill and then
/// fail `settle_check`; the seasoned seller fills, the other gets its
/// asset back.
#[test]
fn ae_p6_an_unseasoned_ask_is_excluded_before_it_can_fail_settlement() {
    let mut env = Env::new(Tp::Spl);
    env.setup_auction();
    let s1 = env.party(BOND, 6 * UNIT);
    // S2 holds 6 units acquired just now: not sellable for 7 days.
    let (k, usdc) = env.bidder(BOND);
    let asset = env.asset_account(&k.pubkey(), 6 * UNIT);
    env.admit(&k.pubkey(), 0);
    let now = env.now();
    env.append_lot_at(&k.pubkey(), 6 * UNIT, now);
    let s2 = Party {
        key: k,
        usdc,
        asset,
    };
    let b1 = env.party(10 * UNIT + BOND, 0);
    let b2 = env.party(10 * UNIT + BOND, 0);
    let crank = env.funded();
    env.open_auction(1, params(Pubkey::default())).unwrap();
    let orders = [
        sealed(Side::Sell, 900_000, 6 * UNIT, 1),
        sealed(Side::Sell, 850_000, 6 * UNIT, 2),
        sealed(Side::Buy, 970_000, 6 * UNIT, 3),
        sealed(Side::Buy, 950_000, 6 * UNIT, 4),
    ];
    env.commit_ask(1, &s1, &orders[0], 6 * UNIT).unwrap();
    env.commit_ask(1, &s2, &orders[1], 6 * UNIT).unwrap();
    env.commit_bid(1, &b1, &orders[2], 10 * UNIT).unwrap();
    env.commit_bid(1, &b2, &orders[3], 10 * UNIT).unwrap();
    env.set_time(COMMIT_END_A);
    for (p, o) in [&s1, &s2, &b1, &b2].iter().zip(&orders) {
        env.reveal(1, p, o).expect("reveal");
    }
    env.set_time(REVEAL_END_A);
    let reasons = screen_reasons(&mut env, 1, 4, &crank);
    assert_eq!(
        reasons,
        vec![None, Some(Exclusion::NotSeasoned), None, None]
    );
    env.clear_only(1, &crank).expect("clear");
    let s = env.auction(1);
    assert_eq!(s.cleared_volume, 6 * UNIT);
    assert_eq!(s.orders[1].alloc, 0, "S2 excluded");
    assert_eq!(s.orders[0].alloc, 6 * UNIT);
    env.allocate(1, &crank, 8).unwrap();
    let fills = env.auction(1).fills;
    assert!(fills.iter().all(|f| f.seller == s1.key.pubkey()));
    for (i, f) in fills.iter().enumerate() {
        let buyer = if f.buyer == b1.key.pubkey() { &b1 } else { &b2 };
        env.settle_auction(1, &crank, i as u32, buyer, &s1)
            .expect("settle");
    }
    env.finish_auction(1, &crank).expect("finish");
    let rs = env.rails_session_of(1);
    env.refund_ask(&crank, &rs, &s2).expect("refund ask");
    assert_eq!(env.bal(&s2.asset), 6 * UNIT, "S2's asset back");
    assert_eq!(env.bal(&s1.asset), 0);
}
