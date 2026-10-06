//! queue-claim (#138, SRS 08 §4.4) — the headline AC-8 demo, end to end
//! against the real `queue_claim.so`, `auction_engine.so`,
//! `liquidity_facility.so`, `mark_engine.so`, `payment_rails.so`,
//! `credit_gate.so` and SAS:
//!
//! 1. a holder's redemption request is attested and becomes a claim
//!    position token, frozen until the issuer consents;
//! 2. **sell path**: the claim sells in an auction-engine session (its own
//!    gate market and mark feed, valued by `quote_pv`); the next window's
//!    fill pays the buyer, not the seller;
//! 3. **pledge path**: a second claim is pledged into liquidity-facility
//!    for a USDC advance; each window fill is swept facility-first through
//!    payment-rails, the advance closes, the claim comes back.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{
    AccountDeserialize, AnchorDeserialize, Discriminator, InstructionData, ToAccountMetas,
};
use anchor_spl::token_2022::spl_token_2022;
use auction_engine::{SessionParams, Side};
use base64::Engine;
use common::*;
use liquidity_facility::{FacilityParams, PositionState};
use payment_rails::accounts as racc;
use queue_claim::{
    accounts as qacc, instruction as qix, ClaimPv, ClaimState, QcError, Request, ScheduledFill,
};
use solana_keypair::Keypair;
use solana_program_pack::Pack;
use solana_signer::Signer;

const UNIT: u64 = 1_000_000;
const SYSTEM: Pubkey = anchor_lang::system_program::ID;

fn qc(e: QcError) -> Fail {
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

/// The issuer side of fills: the queue authority's own USDC / NPCS.
fn issuer_of(env: &mut Env) -> Party {
    let key = env.authority.insecure_clone();
    let mint = env.mint;
    let usdc = env.token_account(&mint, &key.pubkey(), 100 * UNIT);
    let asset = env.asset_account(&key.pubkey(), 0);
    Party { key, usdc, asset }
}

fn qpda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &queue_claim::ID).0
}

/// The queue for the NPCS mint and its PDAs.
struct Q {
    npcs: Pubkey,
    queue: Pubkey,
    attester: Keypair,
}

impl Q {
    fn request(&self, id: u64) -> Pubkey {
        qpda(&[
            queue_claim::REQUEST_SEED,
            self.queue.as_ref(),
            &id.to_le_bytes(),
        ])
    }
    fn npcs_vault(&self, id: u64) -> Pubkey {
        qpda(&[queue_claim::NPCS_VAULT_SEED, self.request(id).as_ref()])
    }
    fn cash_vault(&self, id: u64) -> Pubkey {
        qpda(&[queue_claim::CASH_VAULT_SEED, self.request(id).as_ref()])
    }
    fn claim_mint(&self, id: u64) -> Pubkey {
        qpda(&[queue_claim::CLAIM_MINT_SEED, self.request(id).as_ref()])
    }
    fn holder_claim(&self, id: u64) -> Pubkey {
        qpda(&[queue_claim::CLAIM_ACCOUNT_SEED, self.request(id).as_ref()])
    }
}

fn load_queue(env: &mut Env) -> Q {
    load_root_program(env, queue_claim::ID, "queue_claim");
    let npcs = env.asset_mint;
    let attester = env.funded();
    let queue = qpda(&[queue_claim::QUEUE_SEED, npcs.as_ref()]);
    let auth = env.authority.insecure_clone();
    let ix = Instruction {
        program_id: queue_claim::ID,
        accounts: qacc::InitQueue {
            queue,
            asset_mint: npcs,
            usdc_mint: env.mint,
            authority: auth.pubkey(),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: qix::InitQueue {
            attester: attester.pubkey(),
        }
        .data(),
    };
    env.send(&[ix], &[&auth]).expect("init queue");
    Q {
        npcs,
        queue,
        attester,
    }
}

fn request(env: &mut Env, q: &Q, holder: &Party, qty: u64) -> u64 {
    let id = {
        let a = env.svm.get_account(&q.queue).unwrap();
        queue_claim::Queue::try_deserialize(&mut &a.data[..])
            .unwrap()
            .requests
    };
    let ix = Instruction {
        program_id: queue_claim::ID,
        accounts: qacc::RequestRedemption {
            queue: q.queue,
            request: q.request(id),
            npcs_vault: q.npcs_vault(id),
            signer: queue_claim::signer_pda().0,
            holder: holder.key.pubkey(),
            holder_asset: holder.asset,
            asset_mint: q.npcs,
            asset_token_program: env.asset_tp.id(),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: qix::RequestRedemption {
            qty,
            window_date: T0 + 90 * 86_400,
        }
        .data(),
    };
    env.send(&[ix], &[&holder.key]).expect("request");
    id
}

fn attest_as(env: &mut Env, q: &Q, attester: &Keypair, id: u64, holder: &Pubkey) -> Res {
    let ix = Instruction {
        program_id: queue_claim::ID,
        accounts: qacc::Attest {
            queue: q.queue,
            request: q.request(id),
            claim_mint: q.claim_mint(id),
            holder_claim: q.holder_claim(id),
            cash_vault: q.cash_vault(id),
            holder: *holder,
            signer: queue_claim::signer_pda().0,
            attester: attester.pubkey(),
            usdc_mint: env.mint,
            claim_token_program: env.asset_tp.id(),
            token_program: env.tp.id(),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: qix::Attest {
            rank: 7,
            schedule: vec![
                ScheduledFill {
                    period: 0,
                    qty: 4 * UNIT,
                },
                ScheduledFill {
                    period: 1,
                    qty: 6 * UNIT,
                },
            ],
        }
        .data(),
    };
    env.send(&[ix], &[attester])
}

fn set_flags(env: &mut Env, q: &Q, id: u64, transferable: bool, pledgeable: bool) {
    let auth = env.authority.insecure_clone();
    let ix = Instruction {
        program_id: queue_claim::ID,
        accounts: qacc::SetFlags {
            queue: q.queue,
            request: q.request(id),
            claim_mint: q.claim_mint(id),
            holder_claim: q.holder_claim(id),
            signer: queue_claim::signer_pda().0,
            authority: auth.pubkey(),
            claim_token_program: env.asset_tp.id(),
        }
        .to_account_metas(None),
        data: qix::SetFlags {
            transferable,
            pledgeable,
        }
        .data(),
    };
    env.send(&[ix], &[&auth]).expect("set flags");
}

fn quote_pv(env: &mut Env, q: &Q, id: u64, bps: u64) -> u64 {
    let ix = Instruction {
        program_id: queue_claim::ID,
        accounts: qacc::QuotePv {
            queue: q.queue,
            request: q.request(id),
            asset_feed: feed_pda(&q.npcs),
        }
        .to_account_metas(None),
        data: qix::QuotePv {
            discount_bps_per_period: bps,
        }
        .data(),
    };
    let a = env.authority.insecure_clone();
    let meta = env.send(&[ix], &[&a]).expect("quote");
    events::<ClaimPv>(&meta.logs)[0].pv
}

/// The issuer executes `qty` of request `id` at `price`.
fn record_fill(env: &mut Env, q: &Q, id: u64, issuer: &Party, qty: u64, price: u64) {
    let auth = env.authority.insecure_clone();
    let ix = Instruction {
        program_id: queue_claim::ID,
        accounts: qacc::RecordFill {
            queue: q.queue,
            request: q.request(id),
            npcs_vault: q.npcs_vault(id),
            cash_vault: q.cash_vault(id),
            signer: queue_claim::signer_pda().0,
            authority: auth.pubkey(),
            issuer_usdc: issuer.usdc,
            issuer_asset: issuer.asset,
            asset_mint: q.npcs,
            usdc_mint: env.mint,
            token_program: env.tp.id(),
            asset_token_program: env.asset_tp.id(),
        }
        .to_account_metas(None),
        data: qix::RecordFill {
            fill_qty: qty,
            price,
        }
        .data(),
    };
    env.send(&[ix], &[&auth]).expect("record fill");
}

fn collect(env: &mut Env, q: &Q, id: u64, who: &Keypair, claim: &Pubkey, usdc: &Pubkey) -> Res {
    let ix = Instruction {
        program_id: queue_claim::ID,
        accounts: qacc::Collect {
            queue: q.queue,
            request: q.request(id),
            claim: *claim,
            holder: who.pubkey(),
            cash_vault: q.cash_vault(id),
            holder_usdc: *usdc,
            signer: queue_claim::signer_pda().0,
            usdc_mint: env.mint,
            token_program: env.tp.id(),
        }
        .to_account_metas(None),
        data: qix::Collect {}.data(),
    };
    env.send(&[ix], &[who])
}

fn request_state(env: &Env, q: &Q, id: u64) -> Request {
    let a = env.svm.get_account(&q.request(id)).unwrap();
    Request::try_deserialize(&mut &a.data[..]).unwrap()
}

fn frozen(env: &Env, account: &Pubkey) -> bool {
    let a = env.svm.get_account(account).unwrap();
    let st = spl_token_2022::state::Account::unpack_from_slice(&a.data[..165]).unwrap();
    st.state == spl_token_2022::state::AccountState::Frozen
}

/// Admit `wallet` (already SAS-attested on the shared credential) on the
/// current `env.asset_mint` market and season `qty` there.
fn admit_again(env: &mut Env, wallet: &Pubkey, qty: u64) {
    env.validate(wallet).expect("validate on claim market");
    let now = env.now();
    env.append_lot_at(wallet, qty, now - INVENTORY_AGE);
}

/// Claim feed: the nav-publisher publishes the `quote_pv` value (per whole
/// claim, decimals 0) into the claim mint's own mark feed.
fn publish_claim_mark(env: &mut Env, claim_mint: &Pubkey, publisher: &Keypair, pv: u64) {
    let now = env.now();
    env.set_time(now + 1);
    let nav = env.feed(claim_mint).price_to_pico(pv).unwrap();
    env.publish_nav(claim_mint, publisher, nav)
        .expect("claim mark");
}

#[test]
fn ac8_sell_path_claim_sells_in_an_auction_and_the_fill_pays_the_buyer() {
    let mut env = Env::new(Tp::Spl);
    let publisher = env.setup_auction();
    let q = load_queue(&mut env);
    let holder = env.party(BOND, 10 * UNIT);
    let issuer = issuer_of(&mut env);

    // QC-P1: only the attester attests.
    let id = request(&mut env, &q, &holder, 10 * UNIT);
    assert_eq!(env.bal(&q.npcs_vault(id)), 10 * UNIT);
    let stranger = env.funded();
    assert_eq!(
        attest_as(&mut env, &q, &stranger, id, &holder.key.pubkey()).unwrap_err(),
        qc(QcError::NotAttester)
    );
    let att = q.attester.insecure_clone();
    attest_as(&mut env, &q, &att, id, &holder.key.pubkey()).unwrap();
    let r = request_state(&env, &q, id);
    assert_eq!(r.state, ClaimState::Attested);
    assert_eq!(env.bal(&q.holder_claim(id)), 1);
    assert!(frozen(&env, &q.holder_claim(id)), "no consent yet");

    // QC-P3: model value = 4 × 1.00 + 6 × 1.00 × 0.95 = 9.70 USDC.
    let pv = quote_pv(&mut env, &q, id, 500);
    assert_eq!(pv, 9_700_000);

    // Issuer consent: transferable.
    set_flags(&mut env, &q, id, true, false);
    assert!(!frozen(&env, &q.holder_claim(id)));

    // The claim's own market class: gate market + mark feed (decimals 0).
    let claim = q.claim_mint(id);
    env.asset_mint = claim;
    env.open_gate_market();
    let claim_pub = env.funded();
    env.init_feed(&claim, feed_params(), &claim_pub.pubkey());
    env.feed_admin(
        &claim,
        mark_engine::instruction::SetPrintSource {
            print_source: auction_engine::print_pda().0,
        }
        .data(),
    )
    .unwrap();
    publish_claim_mark(&mut env, &claim, &claim_pub, pv);
    let _ = publisher;
    admit_again(&mut env, &holder.key.pubkey(), 1);
    let seller = Party {
        key: holder.key.insecure_clone(),
        usdc: holder.usdc,
        asset: q.holder_claim(id),
    };
    let b1 = env.party(20 * UNIT + BOND, 0);
    let b2 = env.party(20 * UNIT + BOND, 0);

    let t = env.now();
    let p = SessionParams {
        commit_end: t + 100,
        reveal_end: t + 200,
        rofr_window: 0,
        settle_deadline: t + 400,
        bond: BOND,
        max_cash: 100 * UNIT,
        min_reveals: 3,
        max_discount_bps: 1_000,
        forced_sale: false,
        rofr_authority: Pubkey::default(),
        disclosure_hash: [0xc1; 32],
    };
    env.open_auction(5, p).unwrap();
    let ask = Sealed {
        side: Side::Sell,
        limit: 9_000_000,
        qty: 1,
        nonce: [1; 32],
    };
    let bid1 = Sealed {
        side: Side::Buy,
        limit: 9_500_000,
        qty: 1,
        nonce: [2; 32],
    };
    let bid2 = Sealed {
        side: Side::Buy,
        limit: 9_300_000,
        qty: 1,
        nonce: [3; 32],
    };
    env.commit_ask(5, &seller, &ask, 1).unwrap();
    env.commit_bid(5, &b1, &bid1, 10 * UNIT).unwrap();
    env.commit_bid(5, &b2, &bid2, 10 * UNIT).unwrap();
    env.set_time(p.commit_end);
    env.reveal(5, &seller, &ask).unwrap();
    env.reveal(5, &b1, &bid1).unwrap();
    env.reveal(5, &b2, &bid2).unwrap();
    env.set_time(p.reveal_end);
    let crank = env.funded();
    env.clear_auction(5, &crank).unwrap();
    assert_eq!(env.auction(5).clear_price, 9_500_000);
    env.allocate(5, &crank, 4).unwrap();
    env.settle_auction(5, &crank, 0, &b1, &seller).unwrap();
    env.finish_auction(5, &crank).unwrap();
    assert_eq!(env.bal(&b1.asset), 1, "B1 holds the claim");
    assert_eq!(env.bal(&q.holder_claim(id)), 0);
    let fees = env.bal(&env.fee) + env.bal(&env.reserve);
    assert_eq!(
        env.bal(&holder.usdc),
        9_500_000 - fees,
        "the holder exited today at 9.50 instead of waiting two windows"
    );

    // Window 1 executes 4 of 10 at 1.00: proceeds follow the token.
    record_fill(&mut env, &q, id, &issuer, 4 * UNIT, 1_000_000);
    assert_eq!(
        request_state(&env, &q, id).state,
        ClaimState::PartiallyFilled
    );
    assert_eq!(
        env.bal(&issuer.asset),
        4 * UNIT,
        "redeemed NPCS to the issuer"
    );
    let (hk, hc, hu) = (holder.key.insecure_clone(), q.holder_claim(id), holder.usdc);
    assert_eq!(
        collect(&mut env, &q, id, &hk, &hc, &hu).unwrap_err(),
        qc(QcError::ClaimNotHeld)
    );
    let before = env.bal(&b1.usdc);
    let (bk, ba, bu) = (b1.key.insecure_clone(), b1.asset, b1.usdc);
    collect(&mut env, &q, id, &bk, &ba, &bu).unwrap();
    assert_eq!(env.bal(&b1.usdc), before + 4 * UNIT);
    // Window 2 retires it.
    record_fill(&mut env, &q, id, &issuer, 6 * UNIT, 1_000_000);
    assert_eq!(request_state(&env, &q, id).state, ClaimState::Retired);
    collect(&mut env, &q, id, &bk, &ba, &bu).unwrap();
    assert_eq!(env.bal(&b1.usdc), before + 10 * UNIT);
}

#[test]
fn ac8_pledge_path_advance_then_fills_sweep_facility_first() {
    let mut env = Env::new(Tp::Spl);
    // The NPCS feed + queue first (the facility will be for the claim).
    let npcs = env.asset_mint;
    let holder = env.party(0, 10 * UNIT);
    let issuer = issuer_of(&mut env);
    let funder = env.party(100 * UNIT, 0);
    env.load_mark_engine();
    let npcs_pub = env.funded();
    env.init_feed(&npcs, feed_params(), &npcs_pub.pubkey());
    env.publish_nav(&npcs, &npcs_pub, NAV_ONE).unwrap();
    let q = load_queue(&mut env);
    let id = request(&mut env, &q, &holder, 10 * UNIT);
    let att = q.attester.insecure_clone();
    attest_as(&mut env, &q, &att, id, &holder.key.pubkey()).unwrap();
    let pv = quote_pv(&mut env, &q, id, 500);

    // Not pledgeable without consent.
    let claim = q.claim_mint(id);
    env.asset_mint = claim;
    let claim_pub = env.setup_facility(
        3,
        FacilityParams {
            rate_bps: 1_200,
            advance_rate_bps: 5_000,
            maintenance_ltv_bps: 6_500,
            grace_seconds: 3_600,
            issuer: issuer.key.pubkey(),
            issuer_window: 600,
            backstop: Pubkey::new_unique(),
            backstop_haircut_bps: 1_500,
            backstop_window: 600,
            forced_sale_max_cash: 100 * UNIT,
            lender_workout: Pubkey::new_unique(),
        },
    );
    publish_claim_mark(&mut env, &claim, &claim_pub, pv);
    env.fund_facility(&lf_rails_facility(3), &funder, 100 * UNIT)
        .unwrap();
    env.register(&holder, T0 + 400 * 86_400).unwrap();
    let borrower = Party {
        key: holder.key.insecure_clone(),
        usdc: holder.usdc,
        asset: q.holder_claim(id),
    };
    let pledge = |env: &mut Env, amount: u64| {
        let inner = env.pledge_ix(3, &borrower, 1, amount).accounts;
        let mut ix = Instruction {
            program_id: queue_claim::ID,
            accounts: qacc::Pledge {
                request: q.request(id),
                holder: holder.key.pubkey(),
                facility_program: liquidity_facility::ID,
            }
            .to_account_metas(None),
            data: qix::Pledge { amount }.data(),
        };
        ix.accounts.extend(inner);
        env.send(&[ix], &[&holder.key])
    };
    assert_eq!(
        pledge(&mut env, 4 * UNIT).unwrap_err(),
        qc(QcError::NotPledgeable)
    );
    set_flags(&mut env, &q, id, false, true);
    // 1 claim × 9.70 × 50% = 4.85 USDC max.
    pledge(&mut env, 4 * UNIT).unwrap();
    assert_eq!(env.bal(&holder.usdc), 4 * UNIT);
    assert!(request_state(&env, &q, id).pledge.is_some());

    // A week of interest accrues before the first window.
    let now = env.now();
    env.set_time(now + 7 * 86_400);
    // A pledged claim's proceeds cannot be collected directly…
    let fill1 = |env: &mut Env| record_fill(env, &q, id, &issuer, 4 * UNIT, 1_000_000);
    fill1(&mut env);
    let coll = collateral_pda(&position_pda(&lf_config_pda(3), &holder.key.pubkey()));
    let (hk, hu) = (holder.key.insecure_clone(), holder.usdc);
    assert_eq!(
        collect(&mut env, &q, id, &hk, &coll, &hu).unwrap_err(),
        qc(QcError::ClaimNotHeld)
    );
    // …they sweep facility-first through the rails (QC-P4).
    let rails_facility = lf_rails_facility(3);
    let sweep = |env: &mut Env| {
        let signer = queue_claim::signer_pda().0;
        let curator = liquidity_facility::signer_pda().0;
        let mut ix = Instruction {
            program_id: queue_claim::ID,
            accounts: qacc::SweepPledged {
                queue: q.queue,
                request: q.request(id),
                cash_vault: q.cash_vault(id),
                signer,
                rails_program: payment_rails::ID,
            }
            .to_account_metas(None),
            data: qix::SweepPledged {}.data(),
        };
        let rails: Vec<AccountMeta> = racc::SweepProceeds {
            facility: rails_facility,
            advance: advance_pda(&rails_facility, &holder.key.pubkey()),
            vault: facility_vault_pda(&rails_facility),
            curator,
            proceeds_token: q.cash_vault(id),
            proceeds_authority: signer,
            holder_destination: dest_pda(&holder.key.pubkey()),
            holder_token: holder.usdc,
            usdc_mint: env.mint,
            token_program: env.tp.id(),
        }
        .to_account_metas(None)
        .into_iter()
        .map(|m| AccountMeta {
            is_signer: m.is_signer && m.pubkey != signer,
            ..m
        })
        .collect();
        ix.accounts.extend(rails);
        let a = env.authority.insecure_clone();
        env.send(&[ix], &[&a])
    };
    sweep(&mut env).unwrap();
    // 4 USDC all went to the facility (debt 4 + interest > 4).
    assert_eq!(env.bal(&holder.usdc), 4 * UNIT);
    assert!(request_state(&env, &q, id).pledge.is_some(), "still owed");
    // Window 2: 6 USDC; the facility takes the remainder, the holder the rest.
    record_fill(&mut env, &q, id, &issuer, 6 * UNIT, 1_000_000);
    sweep(&mut env).unwrap();
    let got = env.bal(&holder.usdc) - 4 * UNIT;
    // 6 USDC minus a week of 12% APR on 4 USDC (≈ 9.2k micro-USDC).
    assert!(
        got > 5_980_000 && got < 6 * UNIT,
        "6 USDC minus the accrued interest, got {got}"
    );
    assert!(request_state(&env, &q, id).pledge.is_none(), "discharged");
    assert!(!env.exists(&advance_pda(&rails_facility, &holder.key.pubkey())));
    // The facility releases the claim back to the holder.
    let crank = env.funded();
    env.release(3, &borrower, &crank).unwrap();
    assert_eq!(env.bal(&q.holder_claim(id)), 1);
    assert_eq!(
        env.position(&lf_config_pda(3), &holder.key.pubkey()).state,
        PositionState::Closed
    );
    assert_eq!(request_state(&env, &q, id).state, ClaimState::Retired);
    let _ = npcs;
}

#[test]
fn cancel_returns_the_npcs_and_burns_the_claim() {
    let mut env = Env::new(Tp::Spl);
    env.setup_auction();
    let q = load_queue(&mut env);
    let holder = env.party(0, 10 * UNIT);
    let id = request(&mut env, &q, &holder, 10 * UNIT);
    let att = q.attester.insecure_clone();
    attest_as(&mut env, &q, &att, id, &holder.key.pubkey()).unwrap();
    let ix = Instruction {
        program_id: queue_claim::ID,
        accounts: qacc::Cancel {
            queue: q.queue,
            request: q.request(id),
            holder: holder.key.pubkey(),
            npcs_vault: q.npcs_vault(id),
            holder_asset: holder.asset,
            holder_claim: Some(q.holder_claim(id)),
            claim_mint: Some(q.claim_mint(id)),
            signer: queue_claim::signer_pda().0,
            asset_mint: q.npcs,
            asset_token_program: env.asset_tp.id(),
            claim_token_program: Some(env.asset_tp.id()),
        }
        .to_account_metas(None),
        data: qix::Cancel {}.data(),
    };
    env.send(&[ix], &[&holder.key]).unwrap();
    assert_eq!(env.bal(&holder.asset), 10 * UNIT);
    assert_eq!(env.bal(&q.holder_claim(id)), 0);
    assert_eq!(request_state(&env, &q, id).state, ClaimState::Cancelled);
}
