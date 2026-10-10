//! mark-engine (#137, SRS 08 §4.3) against the real `mark_engine.so`:
//! publisher-set NAV publication, Fresh → Warned → Stale → Fresh by age
//! (AC-2), explicit suspension, hard haircut ceilings, MARK_PROTECTIVE from
//! eligible prints only (AC-4), and the print source as the only print
//! signer.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::AccountDeserialize;
use anchor_lang::{AnchorDeserialize, Discriminator, InstructionData, ToAccountMetas};
use base64::Engine;
use common::*;
use mark_engine::tail::{nav_attestation_data, FeedTail, GuardConfig};
use mark_engine::{
    accounts as macc, instruction as mix, BreakerLevel, FeedMigrated, MarkError, MarkState,
    MarkStateChanged, NavPublication, NavPublished, NavSigned, PrintEligibility, PrintInput,
    PrintPolicy, PrintRecord, PrintRecorded, MAX_STALE_HAIRCUT_BPS,
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

fn publication(env: &Env, nav: u128) -> NavPublication {
    NavPublication {
        nav_pico: nav,
        computed_ts: env.now(),
        model_version: 1,
        inputs_hash: [7; 32],
    }
}

fn set_publishers(env: &mut Env, mint: &Pubkey, keys: Vec<Pubkey>) {
    env.feed_admin(mint, mix::SetPublishers { publishers: keys }.data())
        .expect("set_publishers");
}

fn set_guard(env: &mut Env, mint: &Pubkey, guard: GuardConfig) -> Res {
    env.feed_admin(mint, mix::SetGuard { guard }.data())
}

/// RISK (#202): a production feed can never be turned into a drill feed
/// (nor a drill feed into a production one) after `initialize_feed`.
#[test]
fn drills_are_fixed_at_initialize_feed() {
    let Fx { mut env, mint, .. } = fx();
    let mut p = feed_params();
    p.drills_enabled = false;
    assert_eq!(
        env.feed_admin(&mint, mix::SetParams { params: p }.data())
            .unwrap_err(),
        mark_err(MarkError::DrillsImmutable)
    );
    p.drills_enabled = true;
    p.warned_haircut_bps = 300;
    env.feed_admin(&mint, mix::SetParams { params: p }.data())
        .expect("other params still change");
}

/// ME-P1 / G5 (#202): one publisher alone cannot move the NAV more than
/// the deviation bound (20% by default). An out-of-band NAV is held
/// pending until a second publisher signs the identical publication, or
/// the authority approves that exact publication for manual review.
#[test]
fn an_out_of_band_nav_needs_a_co_signer_or_a_manual_review() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    env.set_time(env.now() + WEEK);
    assert_eq!(
        env.publish_nav(&mint, &publisher, NAV_ONE * 13 / 10)
            .unwrap_err(),
        mark_err(MarkError::NavDeviationTooLarge),
        "30% with no second publisher"
    );
    env.publish_nav(&mint, &publisher, NAV_ONE * 105 / 100)
        .expect("5% is in band");

    let p2 = env.funded();
    set_publishers(&mut env, &mint, vec![publisher.pubkey(), p2.pubkey()]);
    env.set_time(env.now() + WEEK);
    let jump = publication(&env, NAV_ONE * 13 / 10);
    let meta = env.publish_exact(&mint, &publisher, jump, &[]).unwrap();
    let s = &events::<NavSigned>(&meta.logs)[0];
    assert_eq!((s.signatures, s.required, s.out_of_band), (1, 2, true));
    assert_eq!(s.deviation_bps, 2_381, "1.30 vs 1.05, rounded up");
    assert!(events::<NavPublished>(&meta.logs).is_empty(), "pending");
    assert_eq!(env.feed(&mint).nav_pico, NAV_ONE * 105 / 100);
    assert_eq!(
        env.publish_exact(&mint, &publisher, jump, &[]).unwrap_err(),
        mark_err(MarkError::AlreadySigned),
        "one publisher cannot co-sign itself"
    );
    // A different publication does not count towards it.
    let other = NavPublication {
        nav_pico: NAV_ONE * 14 / 10,
        ..jump
    };
    let meta = env.publish_exact(&mint, &p2, other, &[]).unwrap();
    assert_eq!(
        events::<NavSigned>(&meta.logs)[0].signatures,
        1,
        "restarted"
    );
    env.publish_exact(&mint, &publisher, jump, &[]).unwrap();
    let meta = env.publish_exact(&mint, &p2, jump, &[]).unwrap();
    assert_eq!(events::<NavPublished>(&meta.logs).len(), 1, "co-signed");
    assert_eq!(env.feed(&mint).nav_pico, NAV_ONE * 13 / 10);

    // Manual review: the authority approves one exact publication.
    set_publishers(&mut env, &mint, vec![publisher.pubkey()]);
    env.set_time(env.now() + WEEK);
    let back = publication(&env, NAV_ONE);
    assert_eq!(
        env.publish_exact(&mint, &publisher, back, &[]).unwrap_err(),
        mark_err(MarkError::NavDeviationTooLarge)
    );
    env.feed_admin(
        &mint,
        mix::ApproveNavReview {
            nav_pico: back.nav_pico,
            computed_ts: back.computed_ts,
        }
        .data(),
    )
    .unwrap();
    let meta = env.publish_exact(&mint, &publisher, back, &[]).unwrap();
    assert!(events::<NavSigned>(&meta.logs)[0].reviewed);
    assert_eq!(env.feed(&mint).nav_pico, NAV_ONE);
}

/// ME-P1 (#202): with quorum 2 of 2 every NAV, in band or not, is written
/// only once both publishers signed it. A quorum above the set is refused.
#[test]
fn a_quorum_feed_writes_a_nav_only_once_enough_publishers_signed_it() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    let p2 = env.funded();
    set_publishers(&mut env, &mint, vec![publisher.pubkey(), p2.pubkey()]);
    let guard = GuardConfig {
        quorum: 3,
        ..GuardConfig::default()
    };
    assert_eq!(
        set_guard(&mut env, &mint, guard).unwrap_err(),
        mark_err(MarkError::BadGuard)
    );
    set_guard(
        &mut env,
        &mint,
        GuardConfig {
            quorum: 2,
            ..GuardConfig::default()
        },
    )
    .unwrap();
    let p = publication(&env, NAV_ONE);
    let meta = env.publish_exact(&mint, &publisher, p, &[]).unwrap();
    assert_eq!(events::<NavSigned>(&meta.logs)[0].required, 2);
    assert_eq!(env.feed(&mint).publications, 0);
    let outsider = env.funded();
    assert_eq!(
        env.publish_exact(&mint, &outsider, p, &[]).unwrap_err(),
        mark_err(MarkError::NotPublisher)
    );
    env.publish_exact(&mint, &p2, p, &[]).unwrap();
    let feed = env.feed(&mint);
    assert_eq!((feed.publications, feed.nav_pico), (1, NAV_ONE));
    // The set cannot shrink below the quorum.
    assert_eq!(
        env.feed_admin(
            &mint,
            mix::SetPublishers {
                publishers: vec![publisher.pubkey()]
            }
            .data()
        )
        .unwrap_err(),
        mark_err(MarkError::QuorumAboveSet)
    );
}

/// ME-P2 (#202, 06 §4): past 3× cadence the mark is Frozen, which puts the
/// market in Standstill. The `Feed` struct keeps `Stale` so older readers
/// still decode it; the evented state and `state_at` say Frozen.
#[test]
fn a_mark_three_cadences_old_is_frozen_at_standstill() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    let t0 = env.now();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    let frozen_at = t0 + 15 * 86_400 * 3 / 2;
    env.set_time(frozen_at - 1);
    assert_eq!(env.feed(&mint).state_at(env.now()), MarkState::Stale);
    env.set_time(frozen_at);
    let meta = env.refresh_feed(&mint, &publisher).unwrap();
    let c = &events::<MarkStateChanged>(&meta.logs)[0];
    assert_eq!(
        (c.from, c.to, c.breaker),
        (
            MarkState::Fresh,
            MarkState::Frozen,
            BreakerLevel::Standstill
        )
    );
    let feed = env.feed(&mint);
    assert_eq!(feed.last_state, MarkState::Stale, "stored as Stale");
    assert!(feed.is_standstill(env.now()));
    assert!(!feed.allows_liquidation(env.now()));
    // A new mark thaws it.
    let meta = env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    assert_eq!(
        events::<MarkStateChanged>(&meta.logs)[0].from,
        MarkState::Frozen
    );
}

/// #202: a feed created before the tail existed keeps its old behaviour
/// (no deviation bound) until anyone runs `migrate_feed`, which reallocs
/// it once and turns the default guard on.
#[test]
fn a_pre_202_feed_migrates_in_place() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    // Cut the account back to the pre-#202 size.
    let addr = feed_pda(&mint);
    let mut acct = env.svm.get_account(&addr).unwrap();
    acct.data.truncate(FeedTail::OFFSET);
    env.svm.set_account(addr, acct).unwrap();
    assert_eq!(
        set_guard(&mut env, &mint, GuardConfig::default()).unwrap_err(),
        mark_err(MarkError::FeedNotMigrated)
    );
    env.set_time(env.now() + WEEK);
    env.publish_nav(&mint, &publisher, NAV_ONE * 13 / 10)
        .expect("old feed: no bound yet");

    let payer = env.funded();
    let migrate = |env: &mut Env, payer: &Keypair| {
        let ix = Instruction {
            program_id: mark_engine::ID,
            accounts: macc::MigrateFeed {
                feed: addr,
                payer: payer.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: mix::MigrateFeed {}.data(),
        };
        env.send(&[ix], &[payer])
    };
    let meta = migrate(&mut env, &payer).expect("migrate");
    assert_eq!(events::<FeedMigrated>(&meta.logs).len(), 1);
    assert_eq!(
        env.svm.get_account(&addr).unwrap().data.len(),
        FeedTail::FEED_SPACE
    );
    let feed = env.feed(&mint);
    assert_eq!(feed.nav_pico, NAV_ONE * 13 / 10, "state kept");
    env.set_time(env.now() + WEEK);
    assert_eq!(
        env.publish_nav(&mint, &publisher, NAV_ONE).unwrap_err(),
        mark_err(MarkError::NavDeviationTooLarge),
        "the default guard is on"
    );
    let other = env.funded();
    let meta = migrate(&mut env, &other).expect("idempotent");
    assert!(events::<FeedMigrated>(&meta.logs).is_empty());
}

fn print_setup(env: &mut Env, mint: &Pubkey) -> Keypair {
    let source = env.funded();
    env.feed_admin(
        mint,
        mix::SetPrintSource {
            print_source: source.pubkey(),
        }
        .data(),
    )
    .unwrap();
    source
}

fn send_print(env: &mut Env, mint: &Pubkey, source: &Keypair, price: u64, volume: u64) -> Pubkey {
    let session = Pubkey::new_unique();
    let print = PrintInput {
        session,
        price,
        volume,
        n_investor_ids: 3,
        forced_sale: false,
    };
    let ix = record_print_ix(env, mint, &source.pubkey(), print);
    let auth = env.authority.insecure_clone();
    env.send(&[ix], &[&auth, source]).expect("print");
    session
}

fn print_record(env: &Env, mint: &Pubkey, session: &Pubkey) -> PrintRecord {
    let feed = feed_pda(mint);
    let pda = Pubkey::find_program_address(
        &[PrintRecord::SEED, feed.as_ref(), session.as_ref()],
        &mark_engine::ID,
    )
    .0;
    let a = env.svm.get_account(&pda).unwrap();
    PrintRecord::try_deserialize(&mut &a.data[..]).unwrap()
}

/// ME-P3 / M5 (#202): NAV_implied is the volume-weighted mean of recent
/// eligible prints, each print's weight halving every half-life.
#[test]
fn the_implied_mark_is_a_decayed_vwap_of_recent_prints() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    let source = print_setup(&mut env, &mint);
    send_print(&mut env, &mint, &source, 950_000, 1_000_000);
    send_print(&mut env, &mint, &source, 970_000, 3_000_000);
    // (0.95·1 + 0.97·3) / 4 = 0.965.
    assert_eq!(env.feed(&mint).implied_pico, 965_000_000_000);
    // One half-life (warn_after = 8 d) later the old prints weigh half: a
    // new 1-unit print at 0.93 weighs as much as the first two together
    // weigh 2.
    env.set_time(env.now() + 8 * 86_400);
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    send_print(&mut env, &mint, &source, 930_000, 2_000_000);
    // (0.95·0.5 + 0.97·1.5 + 0.93·2) / 4 = 0.9475.
    assert_eq!(env.feed(&mint).implied_pico, 947_500_000_000);
    assert_eq!(
        env.feed(&mint).mark_protective(env.now()).unwrap(),
        947_500_000_000
    );
}

/// ME-P6 (#202): the stored `PrintRecord` keeps only what the market's
/// class discloses — the band floor for BANDED, nothing for DATA_ROOM.
#[test]
fn print_records_store_only_what_the_class_discloses() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    let source = print_setup(&mut env, &mint);
    let public = send_print(&mut env, &mint, &source, 953_210, 2_000_000);
    assert_eq!(print_record(&env, &mint, &public).price, 953_210);
    let mut p = feed_params();
    p.print_policy = PrintPolicy::Banded;
    env.feed_admin(&mint, mix::SetParams { params: p }.data())
        .unwrap();
    let banded = send_print(&mut env, &mint, &source, 953_210, 2_000_000);
    let r = print_record(&env, &mint, &banded);
    assert_eq!((r.price, r.volume), (953_210 / 9_532 * 9_532, 2_000_000));
    p.print_policy = PrintPolicy::DataRoom;
    env.feed_admin(&mint, mix::SetParams { params: p }.data())
        .unwrap();
    let room = send_print(&mut env, &mint, &source, 953_210, 2_000_000);
    let r = print_record(&env, &mint, &room);
    assert_eq!((r.price, r.volume), (0, 0));
    assert_eq!(r.eligibility, PrintEligibility::Eligible, "still marks");
}

/// ME-P1 (#202): a feed that configures a SAS credential + schema takes a
/// NAV only with a live `nundina.nav.v1` attestation, by a publisher-set
/// key, of exactly that publication of this asset.
#[test]
fn an_attested_feed_takes_only_an_attested_nav() {
    let Fx {
        mut env,
        mint,
        publisher,
    } = fx();
    let attester = env.sas.attester.pubkey();
    set_publishers(&mut env, &mint, vec![publisher.pubkey(), attester]);
    let (credential, schema) = (env.sas.credential, env.sas.nav_schema);
    set_guard(
        &mut env,
        &mint,
        GuardConfig {
            sas_credential: credential,
            sas_schema: schema,
            ..GuardConfig::default()
        },
    )
    .unwrap();
    let p = publication(&env, NAV_ONE);
    assert_eq!(
        env.publish_exact(&mint, &publisher, p, &[]).unwrap_err(),
        mark_err(MarkError::AttestationMissing)
    );
    // An attestation of a different NAV does not cover this one.
    let other = Pubkey::new_unique();
    let wrong = NavPublication {
        nav_pico: NAV_ONE + 1,
        ..p
    };
    env.sas_attest(
        schema,
        &other,
        nav_attestation_data(&mint, &wrong),
        ATTESTATION_EXPIRY,
    );
    let wrong_att = env.sas.attestation(&schema, &other);
    assert_eq!(
        env.publish_exact(&mint, &publisher, p, &[wrong_att])
            .unwrap_err(),
        mark_err(MarkError::AttestationInvalid)
    );
    let nonce = Pubkey::new_unique();
    env.sas_attest(
        schema,
        &nonce,
        nav_attestation_data(&mint, &p),
        ATTESTATION_EXPIRY,
    );
    let att = env.sas.attestation(&schema, &nonce);
    env.publish_exact(&mint, &publisher, p, &[att])
        .expect("attested NAV");
    assert_eq!(env.feed(&mint).nav_pico, NAV_ONE);
}
