//! mark-engine (#137, SRS 08 §4.3) against the real `mark_engine.so`:
//! publisher-set NAV publication, Fresh → Warned → Stale → Fresh by age
//! (AC-2), explicit suspension, hard haircut ceilings, MARK_PROTECTIVE from
//! eligible prints only (AC-4), and the print source as the only print
//! signer.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AnchorDeserialize, Discriminator, InstructionData, ToAccountMetas};
use base64::Engine;
use common::*;
use mark_engine::{
    accounts as macc, instruction as mix, MarkError, MarkState, MarkStateChanged, NavPublished,
    PrintEligibility, PrintInput, PrintPolicy, PrintRecord, PrintRecorded, MAX_STALE_HAIRCUT_BPS,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

fn mark_err(e: MarkError) -> Fail {
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
    mint: Pubkey,
    publisher: Keypair,
}

fn fx() -> Fx {
    let mut env = Env::new(Tp::Spl);
    env.load_mark_engine();
    let mint = env.asset_mint;
    let publisher = env.funded();
    env.init_feed(&mint, feed_params(), &publisher.pubkey());
    Fx {
        env,
        mint,
        publisher,
    }
}

fn record_print_ix(env: &Env, mint: &Pubkey, source: &Pubkey, p: PrintInput) -> Instruction {
    let feed = feed_pda(mint);
    Instruction {
        program_id: mark_engine::ID,
        accounts: macc::RecordPrint {
            feed,
            record: Pubkey::find_program_address(
                &[PrintRecord::SEED, feed.as_ref(), p.session.as_ref()],
                &mark_engine::ID,
            )
            .0,
            print_source: *source,
            payer: env.authority.pubkey(),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: mix::RecordPrint { print: p }.data(),
    }
}

#[test]
fn only_the_publisher_set_writes_and_marks_age_through_every_state() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    assert_eq!(env.feed(&mint).state_at(env.now()), MarkState::Stale);
    let outsider = env.funded();
    assert_eq!(
        env.publish_nav(&mint, &outsider, NAV_ONE).unwrap_err(),
        mark_err(MarkError::NotPublisher)
    );

    // AC-2: four weekly marks, one skipped week in the middle.
    let t0 = env.now();
    let meta = env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    let published = events::<NavPublished>(&meta.logs);
    assert_eq!(published.len(), 1);
    assert!(!published[0].admin_drill);
    assert_eq!(
        events::<MarkStateChanged>(&meta.logs)[0].to,
        MarkState::Fresh
    );
    env.set_time(t0 + WEEK);
    env.publish_nav(&mint, &publisher, NAV_ONE + 1_000).unwrap();
    // Skip a week: 8 d without a mark → Warned (evented once).
    env.set_time(t0 + WEEK + 8 * 86_400);
    let meta = env.refresh_feed(&mint, &publisher).unwrap();
    let changed = events::<MarkStateChanged>(&meta.logs);
    assert_eq!(
        (changed[0].from, changed[0].to),
        (MarkState::Fresh, MarkState::Warned)
    );
    let meta = env.refresh_feed(&mint, &publisher).unwrap();
    assert!(events::<MarkStateChanged>(&meta.logs).is_empty(), "once");
    env.set_time(t0 + WEEK + 15 * 86_400);
    let meta = env.refresh_feed(&mint, &publisher).unwrap();
    assert_eq!(
        events::<MarkStateChanged>(&meta.logs)[0].to,
        MarkState::Stale
    );
    let feed = env.feed(&mint);
    assert_eq!(
        feed.nav_effective(env.now()).unwrap(),
        (NAV_ONE + 1_000) * 9 / 10,
        "stale haircut 10%"
    );
    assert!(!feed.allows_liquidation(env.now()));
    // Publishing again returns it to Fresh.
    let meta = env.publish_nav(&mint, &publisher, NAV_ONE + 2_000).unwrap();
    assert_eq!(
        events::<MarkStateChanged>(&meta.logs)[0].to,
        MarkState::Fresh
    );
    let feed = env.feed(&mint);
    assert_eq!(feed.publications, 3);
    assert_eq!(feed.inputs_hash, [7; 32]);
    assert_eq!(feed.model_version, 1);
}

#[test]
fn a_mark_is_never_older_than_the_current_one() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    // Same computed_ts again: not newer.
    assert_eq!(
        env.publish_nav(&mint, &publisher, NAV_ONE).unwrap_err(),
        mark_err(MarkError::NavNotNewer)
    );
    let now = env.now();
    env.set_time(now + 60);
    assert_eq!(
        env.publish_nav(&mint, &publisher, 0).unwrap_err(),
        mark_err(MarkError::ZeroNav)
    );
}

#[test]
fn suspension_is_explicit_and_blocks_publication_and_liquidation() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    // Only the authority suspends.
    let ix = Instruction {
        program_id: mark_engine::ID,
        accounts: macc::FeedAdmin {
            feed: feed_pda(&mint),
            authority: publisher.pubkey(),
        }
        .to_account_metas(None),
        data: mix::Suspend {
            reason_hash: [1; 32],
        }
        .data(),
    };
    assert_eq!(
        env.send(&[ix], &[&publisher]).unwrap_err(),
        mark_err(MarkError::NotAuthority)
    );
    let meta = env
        .feed_admin(
            &mint,
            mix::Suspend {
                reason_hash: [1; 32],
            }
            .data(),
        )
        .unwrap();
    assert_eq!(
        events::<MarkStateChanged>(&meta.logs)[0].to,
        MarkState::Suspended
    );
    let feed = env.feed(&mint);
    assert!(!feed.allows_liquidation(env.now()) && !feed.allows_new_advance(env.now()));
    let now = env.now();
    env.set_time(now + 60);
    assert_eq!(
        env.publish_nav(&mint, &publisher, NAV_ONE).unwrap_err(),
        mark_err(MarkError::Suspended)
    );
    env.feed_admin(&mint, mix::Resume {}.data()).unwrap();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    assert_eq!(env.feed(&mint).state_at(env.now()), MarkState::Fresh);
}

#[test]
fn haircuts_cannot_exceed_the_hard_ceiling() {
    let Fx { mut env, mint, .. } = fx();
    let mut p = feed_params();
    p.stale_haircut_bps = MAX_STALE_HAIRCUT_BPS + 1;
    assert_eq!(
        env.feed_admin(&mint, mix::SetParams { params: p }.data())
            .unwrap_err(),
        mark_err(MarkError::HaircutAboveCeiling)
    );
}

#[test]
fn drill_marks_are_flagged() {
    let Fx { mut env, mint, .. } = fx();
    let meta = env.drill_nav(&mint, NAV_ONE / 2).unwrap();
    assert!(events::<NavPublished>(&meta.logs)[0].admin_drill);
    assert!(env.feed(&mint).last_drill);
}

#[test]
fn eligible_prints_set_the_protective_mark_and_others_are_stored_flagged() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    let source = env.funded();
    let auth = env.authority.insecure_clone();
    let session = Pubkey::new_unique();
    let print = PrintInput {
        session,
        price: 950_000,
        volume: 3_000_000,
        n_investor_ids: 3,
        forced_sale: false,
    };
    // No print source configured yet: nobody can print.
    let ix = record_print_ix(&env, &mint, &source.pubkey(), print);
    assert_eq!(
        env.send(&[ix], &[&auth, &source]).unwrap_err(),
        mark_err(MarkError::NotPrintSource)
    );
    env.feed_admin(
        &mint,
        mix::SetPrintSource {
            print_source: source.pubkey(),
        }
        .data(),
    )
    .unwrap();

    // AC-4: a discount print enters the curve; protective = min.
    let ix = record_print_ix(&env, &mint, &source.pubkey(), print);
    let meta = env.send(&[ix], &[&auth, &source]).unwrap();
    let rec = &events::<PrintRecorded>(&meta.logs)[0];
    assert_eq!(rec.eligibility, PrintEligibility::Eligible);
    assert_eq!(rec.price, Some(950_000));
    let feed = env.feed(&mint);
    assert_eq!(feed.mark_protective(env.now()).unwrap(), NAV_ONE * 95 / 100);
    // One record per session.
    let ix = record_print_ix(&env, &mint, &source.pubkey(), print);
    assert!(env.send(&[ix], &[&auth, &source]).is_err());

    // A forced-sale print is stored but never moves the curve (LF-2).
    let forced = PrintInput {
        session: Pubkey::new_unique(),
        price: 500_000,
        forced_sale: true,
        ..print
    };
    let ix = record_print_ix(&env, &mint, &source.pubkey(), forced);
    let meta = env.send(&[ix], &[&auth, &source]).unwrap();
    assert_eq!(
        events::<PrintRecorded>(&meta.logs)[0].eligibility,
        PrintEligibility::ForcedSale
    );
    let feed = env.feed(&mint);
    assert_eq!(feed.mark_protective(env.now()).unwrap(), NAV_ONE * 95 / 100);
    assert_eq!((feed.prints_accepted, feed.prints_rejected), (1, 1));

    // AGGREGATE_ONLY discloses no price.
    let mut p = feed_params();
    p.print_policy = PrintPolicy::AggregateOnly;
    env.feed_admin(&mint, mix::SetParams { params: p }.data())
        .unwrap();
    let agg = PrintInput {
        session: Pubkey::new_unique(),
        ..print
    };
    let ix = record_print_ix(&env, &mint, &source.pubkey(), agg);
    let meta = env.send(&[ix], &[&auth, &source]).unwrap();
    let rec = &events::<PrintRecorded>(&meta.logs)[0];
    assert_eq!((rec.price, rec.volume), (None, Some(3_000_000)));
}

/// The first caller of `initialize_feed` would own the feed: only the upgrade authority may create one.
#[test]
fn only_the_upgrade_authority_creates_a_feed() {
    let mut env = Env::new(Tp::Spl);
    env.load_mark_engine();
    let mint = env.asset_mint;
    let stranger = env.funded();
    let ix = env.init_feed_ix(&mint, feed_params(), &stranger.pubkey());
    assert_eq!(
        env.send(&[ix], &[&stranger]).unwrap_err(),
        mark_err(MarkError::NotUpgradeAuthority)
    );
    let auth = env.authority.insecure_clone();
    let ix = env.init_feed_ix(&mint, feed_params(), &auth.pubkey());
    env.send(&[ix], &[&auth])
        .expect("the upgrade authority creates it");
}
