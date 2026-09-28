//! R1 exit criteria (PHASES.md): deposit/refund conservation, griefing
//! cases, AC-11 cash side. Every test runs under SPL Token and Token-2022.

mod common;

use common::*;
use payment_rails::{Breaker, RailsError, SessionStatus};
use solana_keypair::Keypair;
use solana_signer::Signer;

fn each(test: impl Fn(Tp)) {
    for tp in Tp::ALL {
        test(tp);
    }
}

fn err(e: RailsError) -> Fail {
    Fail::rails(e)
}

// ── conservation ────────────────────────────────────────────────────────

/// Full lifecycle with three bidders: two reveal, one doesn't. Every
/// micro-USDC is accounted for after every step, and the session ends with
/// zero live escrows, zero totals and every vault/escrow account closed.
#[test]
fn lifecycle_conserves_every_micro_usdc() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let cash = [100_000_000u64, 250_000_000, 42_424_242];
        let bidders: Vec<(Keypair, _)> = (0..3).map(|_| env.bidder(1_000_000_000)).collect();

        let mut tracked = vec![env.reserve, env.fee];
        for (k, t) in &bidders {
            tracked.push(*t);
            let e = escrow_pda(&s, &k.pubkey());
            tracked.push(vault_pda(&e));
        }
        let start = env.total(&tracked);

        for ((k, t), c) in bidders.iter().zip(cash) {
            env.deposit(&s, k, t, c).unwrap();
            let v = vault_pda(&escrow_pda(&s, &k.pubkey()));
            assert_eq!(env.bal(&v), c + BOND, "{tp:?} vault holds cash + bond");
            assert_eq!(env.total(&tracked), start, "{tp:?} conserved after deposit");
        }
        let sess = env.session(&s);
        assert_eq!(sess.open_escrows, 3);
        assert_eq!(sess.total_cash, cash.iter().sum::<u64>());
        assert_eq!(sess.total_bonds, 3 * BOND);

        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &bidders[0].0.pubkey()).unwrap();
        env.mark_revealed(&s, &bidders[1].0.pubkey()).unwrap();

        // Reveals closed: the non-revealer gets cash back, bond → reserve.
        env.set_time(REVEAL_END);
        let (k2, t2) = &bidders[2];
        let before = env.bal(t2);
        let rent_before = env.lamports(&k2.pubkey());
        env.refund(k2, &s, &k2.pubkey(), t2).unwrap();
        assert_eq!(env.bal(t2), before + cash[2], "{tp:?} cash only");
        assert_eq!(env.bal(&env.reserve), BOND, "{tp:?} bond forfeited");
        assert!(
            env.lamports(&k2.pubkey()) > rent_before,
            "{tp:?} rent returned"
        );
        assert_eq!(env.total(&tracked), start);

        // Hard deadline: revealed escrows come back whole (AE-P10).
        env.set_time(SETTLE_DEADLINE);
        for (i, (k, t)) in bidders.iter().take(2).enumerate() {
            let before = env.bal(t);
            env.refund(k, &s, &k.pubkey(), t).unwrap();
            assert_eq!(env.bal(t), before + cash[i] + BOND, "{tp:?} full refund");
            assert_eq!(env.total(&tracked), start);
        }

        let sess = env.session(&s);
        assert_eq!(
            (sess.open_escrows, sess.total_cash, sess.total_bonds),
            (0, 0, 0),
            "{tp:?} session drained"
        );
        for (k, _) in &bidders {
            let e = escrow_pda(&s, &k.pubkey());
            assert!(!env.exists(&e), "{tp:?} escrow closed");
            assert!(!env.exists(&vault_pda(&e)), "{tp:?} vault closed");
        }
        assert_eq!(env.bal(&env.fee), 0, "{tp:?} R1 never touches fees");
    });
}

// ── refund timing ───────────────────────────────────────────────────────

#[test]
fn refunds_locked_until_their_deadline() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        let (b, tb) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();
        env.deposit(&s, &b, &tb, 100_000_000).unwrap();
        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &a.pubkey()).unwrap();

        for now in [COMMIT_END, REVEAL_END - 1] {
            env.set_time(now);
            assert_eq!(
                env.refund(&a, &s, &a.pubkey(), &ta).unwrap_err(),
                err(RailsError::DeadlineNotReached)
            );
            assert_eq!(
                env.refund(&b, &s, &b.pubkey(), &tb).unwrap_err(),
                err(RailsError::DeadlineNotReached)
            );
        }
        // Revealed stays locked (awaiting settlement) until the hard deadline.
        env.set_time(SETTLE_DEADLINE - 1);
        assert_eq!(
            env.refund(&a, &s, &a.pubkey(), &ta).unwrap_err(),
            err(RailsError::DeadlineNotReached)
        );
        env.set_time(SETTLE_DEADLINE);
        env.refund(&a, &s, &a.pubkey(), &ta).unwrap();
    });
}

// ── griefing ────────────────────────────────────────────────────────────

/// Anyone may trigger a refund (dead-crank safety), but nobody can redirect
/// it: the destination must belong to the escrow owner.
#[test]
fn third_party_refund_cannot_redirect_cash() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (victim, tv) = env.bidder(500_000_000);
        env.deposit(&s, &victim, &tv, 200_000_000).unwrap();
        // Revealed bidder: at the hard deadline the refund is cash + bond.
        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &victim.pubkey()).unwrap();
        env.set_time(SETTLE_DEADLINE);

        let (attacker, t_att) = env.bidder(0);
        assert_eq!(
            env.refund(&attacker, &s, &victim.pubkey(), &t_att)
                .unwrap_err(),
            err(RailsError::NotEscrowOwner),
            "{tp:?}"
        );
        // Claiming to be the owner doesn't help: the escrow PDA is seeded by
        // the real owner, so the attacker's own escrow simply doesn't exist.
        assert!(matches!(
            env.refund(&attacker, &s, &attacker.pubkey(), &t_att)
                .unwrap_err(),
            Fail::Code(_) | Fail::Other(_)
        ));

        // Triggering it for the victim works — and pays the victim.
        let before = env.bal(&tv);
        env.refund(&attacker, &s, &victim.pubkey(), &tv).unwrap();
        assert_eq!(env.bal(&tv), before + 200_000_000 + BOND, "{tp:?}");
        assert_eq!(env.bal(&t_att), 0, "{tp:?} attacker gains nothing");
    });
}

/// A dust donation into a vault must not be able to block its close.
#[test]
fn dust_donation_cannot_block_refund() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (owner, t) = env.bidder(500_000_000);
        env.deposit(&s, &owner, &t, 100_000_000).unwrap();
        let (griefer, tg) = env.bidder(10);
        let vault = vault_pda(&escrow_pda(&s, &owner.pubkey()));
        env.raw_transfer(&tg, &vault, &griefer, 1).unwrap();

        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &owner.pubkey()).unwrap();
        env.set_time(SETTLE_DEADLINE);
        let before = env.bal(&t);
        env.refund(&griefer, &s, &owner.pubkey(), &t).unwrap();
        assert_eq!(
            env.bal(&t),
            before + 100_000_000 + BOND + 1,
            "{tp:?} dust goes to owner"
        );
        assert!(!env.exists(&vault), "{tp:?} vault closed anyway");
        assert_eq!(env.session(&s).total_cash, 0);
    });
}

#[test]
fn bond_only_ever_reaches_the_snapshotted_reserve() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (owner, t) = env.bidder(500_000_000);
        env.deposit(&s, &owner, &t, 100_000_000).unwrap();
        env.set_time(REVEAL_END); // unrevealed ⇒ bond forfeits on refund

        let (thief, _) = env.bidder(0);
        let mint = env.mint;
        let fake_reserve = env.token_account(&mint, &thief.pubkey(), 0);
        assert_eq!(
            env.refund_to(&thief, &s, &owner.pubkey(), &t, &fake_reserve)
                .unwrap_err(),
            err(RailsError::ReserveMismatch),
            "{tp:?}"
        );
        env.refund(&thief, &s, &owner.pubkey(), &t).unwrap();
        assert_eq!(env.bal(&fake_reserve), 0);
        assert_eq!(env.bal(&env.reserve), BOND);
    });
}

#[test]
fn escrow_cannot_be_refunded_twice() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (owner, t) = env.bidder(500_000_000);
        env.deposit(&s, &owner, &t, 100_000_000).unwrap();
        env.set_time(SETTLE_DEADLINE);
        env.refund(&owner, &s, &owner.pubkey(), &t).unwrap();
        let after = env.bal(&t);
        assert!(
            env.refund(&owner, &s, &owner.pubkey(), &t).is_err(),
            "{tp:?}"
        );
        assert_eq!(env.bal(&t), after, "{tp:?} no double pay");
        assert_eq!(env.session(&s).open_escrows, 0);
    });
}

// ── AC-11: refunds survive every halt ───────────────────────────────────

#[test]
fn refunds_work_while_paused_and_depegged() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();

        env.set_breaker(Breaker::Pause, true).unwrap();
        env.set_breaker(Breaker::Depeg, true).unwrap();
        let cfg = env.config();
        assert!(cfg.paused && cfg.depeg_breaker);

        // New money is blocked …
        let (b, tb) = env.bidder(500_000_000);
        assert_eq!(
            env.deposit(&s, &b, &tb, 1).unwrap_err(),
            err(RailsError::Paused),
            "{tp:?}"
        );
        let op = env.authority.insecure_clone();
        assert_eq!(
            env.open_session_as(
                &op,
                2,
                (COMMIT_END, REVEAL_END, SETTLE_DEADLINE),
                BOND,
                MAX_CASH
            )
            .unwrap_err(),
            err(RailsError::Paused)
        );
        env.set_breaker(Breaker::Pause, false).unwrap();
        assert_eq!(
            env.deposit(&s, &b, &tb, 1).unwrap_err(),
            err(RailsError::DepegBreakerTripped),
            "{tp:?}"
        );
        env.set_breaker(Breaker::Pause, true).unwrap();

        // … refunds are not: failed session → immediate, by a stranger.
        env.fail_session_as(&op, &s).unwrap();
        let stranger = env.funded();
        let before = env.bal(&ta);
        env.refund(&stranger, &s, &a.pubkey(), &ta).unwrap();
        assert_eq!(env.bal(&ta), before + 100_000_000 + BOND, "{tp:?}");
    });
}

// ── failure and forfeit semantics ───────────────────────────────────────

#[test]
fn failure_before_reveals_close_forfeits_nothing() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();
        env.set_time(COMMIT_END + 10); // mid-reveal, a has not revealed
        let op = env.authority.insecure_clone();
        env.fail_session_as(&op, &s).unwrap();
        assert_eq!(env.session(&s).status, SessionStatus::Failed);
        assert_eq!(
            env.fail_session_as(&op, &s).unwrap_err(),
            err(RailsError::SessionNotOpen)
        );

        env.set_time(REVEAL_END + 50); // even after reveals would have closed
        let before = env.bal(&ta);
        env.refund(&a, &s, &a.pubkey(), &ta).unwrap();
        assert_eq!(
            env.bal(&ta),
            before + 100_000_000 + BOND,
            "{tp:?} bond returned"
        );
        assert_eq!(env.bal(&env.reserve), 0);
    });
}

#[test]
fn failure_after_reveals_close_still_forfeits_non_revealers() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        let (b, tb) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();
        env.deposit(&s, &b, &tb, 100_000_000).unwrap();
        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &a.pubkey()).unwrap();
        env.set_time(REVEAL_END + 1); // e.g. no cross ≥ floor
        let op = env.authority.insecure_clone();
        env.fail_session_as(&op, &s).unwrap();

        let (ba, bb) = (env.bal(&ta), env.bal(&tb));
        env.refund(&a, &s, &a.pubkey(), &ta).unwrap();
        env.refund(&b, &s, &b.pubkey(), &tb).unwrap();
        assert_eq!(
            env.bal(&ta),
            ba + 100_000_000 + BOND,
            "{tp:?} revealer whole"
        );
        assert_eq!(
            env.bal(&tb),
            bb + 100_000_000,
            "{tp:?} non-revealer loses bond"
        );
        assert_eq!(env.bal(&env.reserve), BOND);
    });
}

#[test]
fn forfeit_bond_is_permissionless_and_exact() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        let (b, tb) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();
        env.deposit(&s, &b, &tb, 100_000_000).unwrap();
        env.set_time(COMMIT_END);
        env.mark_revealed(&s, &a.pubkey()).unwrap();
        let crank = env.funded();

        env.set_time(REVEAL_END - 1);
        assert_eq!(
            env.forfeit_bond(&crank, &s, &b.pubkey()).unwrap_err(),
            err(RailsError::BondNotForfeitable),
            "{tp:?} too early"
        );
        env.set_time(REVEAL_END);
        assert_eq!(
            env.forfeit_bond(&crank, &s, &a.pubkey()).unwrap_err(),
            err(RailsError::BondNotForfeitable),
            "{tp:?} revealer protected"
        );
        env.forfeit_bond(&crank, &s, &b.pubkey()).unwrap();
        assert_eq!(env.bal(&env.reserve), BOND);
        assert_eq!(
            env.forfeit_bond(&crank, &s, &b.pubkey()).unwrap_err(),
            err(RailsError::BondNotForfeitable),
            "{tp:?} no double forfeit"
        );
        assert_eq!(env.session(&s).total_bonds, BOND, "only a's bond left");

        // Refund after forfeit returns cash only, reserve untouched.
        let before = env.bal(&tb);
        env.refund(&crank, &s, &b.pubkey(), &tb).unwrap();
        assert_eq!(env.bal(&tb), before + 100_000_000, "{tp:?}");
        assert_eq!(env.bal(&env.reserve), BOND);
        // The revealer's escrow is untouched by all of the above.
        assert_eq!(
            env.bal(&vault_pda(&escrow_pda(&s, &a.pubkey()))),
            100_000_000 + BOND
        );
        assert_eq!(env.bal(&ta), 400_000_000 - BOND);
    });
}

// ── deposit guards ──────────────────────────────────────────────────────

#[test]
fn deposit_guards() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(2 * MAX_CASH);

        assert_eq!(
            env.deposit(&s, &a, &ta, 0).unwrap_err(),
            err(RailsError::ZeroAmount),
            "{tp:?}"
        );
        assert_eq!(
            env.deposit(&s, &a, &ta, MAX_CASH + 1).unwrap_err(),
            err(RailsError::CashCapExceeded),
            "{tp:?}"
        );
        // Foreign mint (same decimals) is refused.
        let other = env.new_mint(6);
        let t_other = env.token_account(&other, &a.pubkey(), 1_000_000_000);
        assert_eq!(
            env.deposit_with_mint(&s, &a, &t_other, other, 1_000)
                .unwrap_err(),
            err(RailsError::MintMismatch),
            "{tp:?}"
        );
        // Insufficient funds fails in the token program; nothing recorded.
        let (poor, tp_poor) = env.bidder(BOND);
        assert!(env.deposit(&s, &poor, &tp_poor, 1).is_err());
        assert_eq!(env.session(&s).open_escrows, 0);

        env.deposit(&s, &a, &ta, MAX_CASH).unwrap();
        // One escrow per (session, owner).
        assert!(env.deposit(&s, &a, &ta, 1).is_err(), "{tp:?} second escrow");

        env.set_time(COMMIT_END);
        let (late, tl) = env.bidder(500_000_000);
        assert_eq!(
            env.deposit(&s, &late, &tl, 1).unwrap_err(),
            err(RailsError::OutsideWindow),
            "{tp:?}"
        );
        assert_eq!(env.session(&s).open_escrows, 1);
    });
}

// ── operator authority ──────────────────────────────────────────────────

#[test]
fn only_the_operator_steers_sessions() {
    each(|tp| {
        let mut env = Env::new(tp);
        let rogue = env.funded();
        let sched = (COMMIT_END, REVEAL_END, SETTLE_DEADLINE);
        assert_eq!(
            env.open_session_as(&rogue, 9, sched, BOND, MAX_CASH)
                .unwrap_err(),
            err(RailsError::NotOperator),
            "{tp:?}"
        );
        assert_eq!(
            env.set_operator_as(&rogue, rogue.pubkey()).unwrap_err(),
            err(RailsError::Unauthorized)
        );

        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();

        env.set_time(COMMIT_END);
        assert_eq!(
            env.mark_revealed_as(&rogue, &s, &a.pubkey()).unwrap_err(),
            err(RailsError::NotOperator)
        );
        assert_eq!(
            env.fail_session_as(&rogue, &s).unwrap_err(),
            err(RailsError::NotOperator)
        );
        env.mark_revealed(&s, &a.pubkey()).unwrap();
        assert_eq!(
            env.mark_revealed(&s, &a.pubkey()).unwrap_err(),
            err(RailsError::AlreadyRevealed)
        );

        // Hand-over: new operator opens sessions; the old one can't.
        let engine = env.funded();
        let auth = env.authority.insecure_clone();
        env.set_operator_as(&auth, engine.pubkey()).unwrap();
        env.set_time(common::T0);
        let late = (COMMIT_END, REVEAL_END, SETTLE_DEADLINE);
        assert_eq!(
            env.open_session_as(&auth, 2, late, BOND, MAX_CASH)
                .unwrap_err(),
            err(RailsError::NotOperator)
        );
        env.open_session_as(&engine, 2, late, BOND, MAX_CASH)
            .unwrap();
        // The old session keeps its snapshotted operator (L5).
        assert_eq!(env.session(&s).operator, auth.pubkey());
    });
}

#[test]
fn reveals_only_inside_the_reveal_window() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();
        assert_eq!(
            env.mark_revealed(&s, &a.pubkey()).unwrap_err(),
            err(RailsError::OutsideWindow),
            "{tp:?} during commit"
        );
        env.set_time(REVEAL_END);
        assert_eq!(
            env.mark_revealed(&s, &a.pubkey()).unwrap_err(),
            err(RailsError::OutsideWindow),
            "{tp:?} after reveal"
        );
    });
}

#[test]
fn session_schedule_and_amounts_validated() {
    each(|tp| {
        let mut env = Env::new(tp);
        let op = env.authority.insecure_clone();
        let bad = [
            (T0, REVEAL_END, SETTLE_DEADLINE),         // commit_end == now
            (COMMIT_END, COMMIT_END, SETTLE_DEADLINE), // commit == reveal
            (COMMIT_END, REVEAL_END, REVEAL_END),      // reveal == settle
            (COMMIT_END, SETTLE_DEADLINE, REVEAL_END), // out of order
        ];
        for (i, sched) in bad.into_iter().enumerate() {
            assert_eq!(
                env.open_session_as(&op, 100 + i as u64, sched, BOND, MAX_CASH)
                    .unwrap_err(),
                err(RailsError::InvalidSchedule),
                "{tp:?} {sched:?}"
            );
        }
        let good = (COMMIT_END, REVEAL_END, SETTLE_DEADLINE);
        assert_eq!(
            env.open_session_as(&op, 7, good, 0, MAX_CASH).unwrap_err(),
            err(RailsError::ZeroAmount)
        );
        assert_eq!(
            env.open_session_as(&op, 7, good, BOND, 0).unwrap_err(),
            err(RailsError::ZeroAmount)
        );
        let s = env.open_session_as(&op, 7, good, BOND, MAX_CASH).unwrap();
        let sess = env.session(&s);
        assert_eq!(sess.reserve, env.reserve, "reserve snapshotted");
        assert_eq!(sess.usdc_mint, env.mint, "mint snapshotted");
        assert!(
            env.open_session_as(&op, 7, good, BOND, MAX_CASH).is_err(),
            "id reuse"
        );
    });
}

// ── shortfall (#62) ─────────────────────────────────────────────────────

/// Sets a token account's balance directly: simulates a settlement-mint
/// permanent delegate or clawback pulling from an escrow vault.
fn shrink_vault(env: &mut Env, vault: &anchor_lang::prelude::Pubkey, amount: u64) {
    let mut a = env.svm.get_account(vault).expect("vault");
    a.data[64..72].copy_from_slice(&amount.to_le_bytes());
    env.svm.set_account(*vault, a).unwrap();
}

/// A vault drained below its recorded amount must not block the refund
/// (AE-P10 / doc 06 §11 H5): the owner gets what's left, everything closes.
#[test]
fn short_cash_vault_still_refunds_and_closes() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();
        let op = env.authority.insecure_clone();
        env.fail_session_as(&op, &s).unwrap();

        let e = escrow_pda(&s, &a.pubkey());
        shrink_vault(&mut env, &vault_pda(&e), 60_000_000);
        let before = env.bal(&ta);
        let crank = env.funded();
        env.refund(&crank, &s, &a.pubkey(), &ta)
            .expect("refund completes despite the shortfall");
        assert_eq!(
            env.bal(&ta),
            before + 60_000_000,
            "{tp:?} owner gets what is held"
        );
        assert!(
            !env.exists(&vault_pda(&e)) && !env.exists(&e),
            "{tp:?} closed"
        );
        let sess = env.session(&s);
        assert_eq!(
            (sess.open_escrows, sess.total_cash, sess.total_bonds),
            (0, 0, 0)
        );
    });
}

/// With a forfeit due and fewer micros held than the bond, the reserve gets
/// everything held and the refund still closes (no underflow, no revert).
#[test]
fn short_vault_pays_forfeit_first_without_underflow() {
    each(|tp| {
        let mut env = Env::new(tp);
        let s = env.open_session(1);
        let (a, ta) = env.bidder(500_000_000);
        env.deposit(&s, &a, &ta, 100_000_000).unwrap();
        env.set_time(REVEAL_END); // never revealed ⇒ bond forfeits
        let e = escrow_pda(&s, &a.pubkey());
        shrink_vault(&mut env, &vault_pda(&e), BOND - 1);
        let before = env.bal(&ta);
        env.set_time(SETTLE_DEADLINE);
        let crank = env.funded();
        env.refund(&crank, &s, &a.pubkey(), &ta).unwrap();
        assert_eq!(env.bal(&env.reserve), BOND - 1, "{tp:?}");
        assert_eq!(env.bal(&ta), before, "{tp:?}");
        assert!(!env.exists(&e));
    });
}
