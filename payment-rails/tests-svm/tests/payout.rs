//! R3 exit criteria (PHASES.md): a non-allowlisted destination is rejected
//! with a typed reason; the coupon waterfall sums exactly to cash in. Plus
//! the facility cash leg (advance / interest-first repay) and the QC-P4
//! sweep order (facility first, then holder). Every test runs under SPL
//! Token and Token-2022 USDC.

mod common;

use common::*;
use payment_rails::{Breaker, CouponParams, CouponSplit, RailsError, MAX_RATE_BPS};
use solana_signer::Signer;

const YEAR: i64 = 31_536_000;
const FAR: i64 = T0 + 10 * YEAR; // attestation expiry used by default
const USDC: u64 = 1_000_000;

fn each(test: impl Fn(Tp)) {
    for tp in Tp::ALL {
        test(tp);
    }
}

fn err(e: RailsError) -> Fail {
    Fail::rails(e)
}

/// Env + a facility (12% APR) funded with 10,000 USDC by a treasury party.
fn with_facility(tp: Tp) -> (Env, anchor_lang::prelude::Pubkey, Party) {
    let mut env = Env::new(tp);
    let fac = env.open_facility(1, 1_200);
    let treasury = env.party(10_000 * USDC, 0);
    env.fund_facility(&fac, &treasury, 10_000 * USDC).unwrap();
    (env, fac, treasury)
}

// ── exit criterion 1: typed rejection of non-allowlisted destinations ───

#[test]
fn non_allowlisted_destination_rejected_with_typed_reason() {
    each(|tp| {
        let (mut env, fac, _) = with_facility(tp);
        let curator = env.authority.insecure_clone();
        let bob = env.party(0, 0);
        let bob_key = bob.key.pubkey();
        let vault_before = env.bal(&facility_vault_pda(&fac));

        // 1. Never registered: the PDA doesn't exist → typed, not a framework error.
        assert_eq!(
            env.advance(&fac, &bob, 100 * USDC).unwrap_err(),
            err(RailsError::DestinationNotAllowlisted),
            "{tp:?} unregistered"
        );
        // 2. Some other program-owned account passed as the registry entry.
        let cfg = env.config;
        assert_eq!(
            env.advance_to(&curator, &fac, &bob_key, &cfg, &bob.usdc, 100 * USDC)
                .unwrap_err(),
            err(RailsError::DestinationNotAllowlisted),
            "{tp:?} wrong account type"
        );
        // 3. Someone else's (valid) entry.
        let alice = env.party(0, 0);
        env.register(&alice, FAR).unwrap();
        let alice_dest = dest_pda(&alice.key.pubkey());
        assert_eq!(
            env.advance_to(
                &curator,
                &fac,
                &bob_key,
                &alice_dest,
                &alice.usdc,
                100 * USDC
            )
            .unwrap_err(),
            err(RailsError::DestinationNotAllowlisted),
            "{tp:?} another wallet's entry"
        );
        // 4. Registered, but paying into a different account (even Bob's own).
        env.register(&bob, FAR).unwrap();
        let mint = env.mint;
        let bob_other = env.token_account(&mint, &bob_key, 0);
        let bob_dest = dest_pda(&bob_key);
        assert_eq!(
            env.advance_to(&curator, &fac, &bob_key, &bob_dest, &bob_other, 100 * USDC)
                .unwrap_err(),
            err(RailsError::DestinationNotAllowlisted),
            "{tp:?} unregistered token account"
        );
        // 5. Revoked.
        env.revoke(&bob_key).unwrap();
        assert_eq!(
            env.advance(&fac, &bob, 100 * USDC).unwrap_err(),
            err(RailsError::DestinationRevoked)
        );
        // 6. Expired.
        env.renew(&bob_key, &bob.usdc, T0 + 50).unwrap();
        env.set_time(T0 + 50);
        assert_eq!(
            env.advance(&fac, &bob, 100 * USDC).unwrap_err(),
            err(RailsError::DestinationExpired)
        );

        assert_eq!(
            env.bal(&facility_vault_pda(&fac)),
            vault_before,
            "{tp:?} nothing left the vault"
        );
        // Renewed → pays.
        env.renew(&bob_key, &bob.usdc, FAR).unwrap();
        env.advance(&fac, &bob, 100 * USDC).unwrap();
        assert_eq!(env.bal(&bob.usdc), 100 * USDC, "{tp:?}");
    });
}

#[test]
fn registry_is_registrar_only_hash_only() {
    each(|tp| {
        let mut env = Env::new(tp);
        let p = env.party(0, 0);
        let rogue = env.funded();
        let owner = p.key.pubkey();
        assert_eq!(
            env.register_as(&rogue, &owner, &p.usdc, FAR).unwrap_err(),
            err(RailsError::NotRegistrar),
            "{tp:?}"
        );
        // The payout account must be the owner's own settlement-mint account.
        let stranger = env.party(0, 0);
        let reg = env.authority.insecure_clone();
        assert_eq!(
            env.register_as(&reg, &owner, &stranger.usdc, FAR)
                .unwrap_err(),
            err(RailsError::NotEscrowOwner)
        );
        assert_eq!(
            env.register_as(&reg, &owner, &p.asset, FAR).unwrap_err(),
            err(RailsError::MintMismatch)
        );
        assert_eq!(
            env.register_as(&reg, &owner, &p.usdc, T0).unwrap_err(),
            err(RailsError::DestinationExpired)
        );

        env.register(&p, FAR).unwrap();
        let d = env.destination(&owner);
        assert_eq!((d.owner, d.token_account, d.active), (owner, p.usdc, true));
        assert_eq!(d.attestation_ref_hash, [0xAB; 32], "only a hash is stored");
        assert_eq!(
            env.revoke_as(&rogue, &owner).unwrap_err(),
            err(RailsError::NotRegistrar)
        );

        // Registrar hand-over: old key loses the power, new key has it.
        let attester = env.funded();
        env.set_registrar(attester.pubkey()).unwrap();
        assert_eq!(
            env.revoke(&owner).unwrap_err(),
            err(RailsError::NotRegistrar)
        );
        env.revoke_as(&attester, &owner).unwrap();
        assert!(!env.destination(&owner).active);
    });
}

// ── facility cash leg ───────────────────────────────────────────────────

#[test]
fn facility_interest_is_exact_and_repayment_interest_first() {
    each(|tp| {
        let (mut env, fac, treasury) = with_facility(tp);
        let bob = env.party(0, 0);
        env.register(&bob, FAR).unwrap();
        env.advance(&fac, &bob, 1_000 * USDC).unwrap();
        assert_eq!(env.bal(&bob.usdc), 1_000 * USDC, "{tp:?} advance paid");
        assert_eq!(env.facility(&fac).principal_outstanding, 1_000 * USDC);

        let vault = facility_vault_pda(&fac);
        let accounts = [bob.usdc, vault, treasury.usdc];
        let total0 = env.total(&accounts);

        // One year at 12% on 1,000 USDC = exactly 120 USDC (credit-math).
        env.set_time(T0 + YEAR);
        // Bob tops up so he can pay interest (outside the system: +200).
        let top_up = env.party(200 * USDC, 0);
        env.raw_transfer(&top_up.usdc, &bob.usdc, &top_up.key, 200 * USDC)
            .unwrap();
        let total0 = total0 + 200 * USDC;

        env.repay(&fac, &bob.key.pubkey(), &bob, 50 * USDC).unwrap();
        let a = env.advance_state(&fac, &bob.key.pubkey());
        assert_eq!(
            (a.interest, a.principal),
            (70 * USDC, 1_000 * USDC),
            "{tp:?} interest first"
        );

        // Overpay: only what is owed (70 + 1,000) is taken.
        let before = env.bal(&bob.usdc);
        env.repay(&fac, &bob.key.pubkey(), &bob, 1_150 * USDC)
            .unwrap();
        assert_eq!(
            before - env.bal(&bob.usdc),
            1_070 * USDC,
            "{tp:?} capped at owed"
        );
        assert!(
            !env.exists(&advance_pda(&fac, &bob.key.pubkey())),
            "{tp:?} advance closed"
        );

        let f = env.facility(&fac);
        assert_eq!(f.interest_collected, 120 * USDC, "{tp:?} exactly 12%");
        assert_eq!(f.principal_outstanding, 0);
        assert_eq!(env.bal(&vault), 10_000 * USDC + 120 * USDC);
        assert_eq!(env.total(&accounts), total0, "{tp:?} conserved");
    });
}

/// #94: doc 06 §11 H1 is enforced, not a dead constant. The rate may not
/// exceed `credit_math::MAX_ADVANCE_RATE_BPS` (50%), and the advance may not
/// exceed the stated collateral mark × rate. Both are recorded on the
/// `Advance`, so the bound an advance was made under is auditable.
#[test]
fn advance_is_bounded_by_the_h1_ceiling() {
    each(|tp| {
        let (mut env, fac, _) = with_facility(tp);
        let bob = env.party(0, 0);
        env.register(&bob, FAR).unwrap();
        let cur = env.authority.insecure_clone();
        let owner = bob.key.pubkey();
        let d = dest_pda(&owner);
        let value = 1_000 * USDC;
        let adv = |env: &mut Env, amount: u64, rate: u64| {
            env.advance_full(&cur, &fac, &owner, &d, &bob.usdc, amount, value, rate)
        };

        assert_eq!(
            adv(&mut env, USDC, MAX_ADVANCE_RATE_BPS + 1).unwrap_err(),
            err(RailsError::AdvanceRateTooHigh),
            "{tp:?} above the 50% ceiling, whatever the amount"
        );
        assert_eq!(
            adv(&mut env, USDC, u64::MAX).unwrap_err(),
            err(RailsError::AdvanceRateTooHigh)
        );
        assert_eq!(
            adv(&mut env, 500 * USDC + 1, MAX_ADVANCE_RATE_BPS).unwrap_err(),
            err(RailsError::AdvanceExceedsRate),
            "{tp:?} one micro over mark × 50%"
        );
        assert_eq!(
            adv(&mut env, 250 * USDC + 1, 2_500).unwrap_err(),
            err(RailsError::AdvanceExceedsRate),
            "{tp:?} a curator rate below the ceiling binds too"
        );
        assert!(!env.exists(&advance_pda(&fac, &owner)), "nothing advanced");

        adv(&mut env, 250 * USDC, 2_500).expect("exactly mark × rate");
        let a = env.advance_state(&fac, &owner);
        assert_eq!(
            (a.principal, a.collateral_value, a.advance_rate_bps),
            (250 * USDC, value, 2_500),
            "{tp:?} terms recorded on the advance"
        );
    });
}

#[test]
fn facility_guards() {
    each(|tp| {
        let (mut env, fac, _) = with_facility(tp);
        let bob = env.party(0, 0);
        env.register(&bob, FAR).unwrap();
        let rogue = env.funded();
        let owner = bob.key.pubkey();
        let d = dest_pda(&owner);

        assert_eq!(
            env.advance_to(&rogue, &fac, &owner, &d, &bob.usdc, USDC)
                .unwrap_err(),
            err(RailsError::NotCurator),
            "{tp:?}"
        );
        assert_eq!(
            env.advance(&fac, &bob, 10_001 * USDC).unwrap_err(),
            err(RailsError::FacilityUnderfunded)
        );
        assert_eq!(
            env.advance(&fac, &bob, 0).unwrap_err(),
            err(RailsError::ZeroAmount)
        );
        env.set_breaker(Breaker::Depeg, true).unwrap();
        assert_eq!(
            env.advance(&fac, &bob, USDC).unwrap_err(),
            err(RailsError::DepegBreakerTripped)
        );
        env.set_breaker(Breaker::Depeg, false).unwrap();
        env.advance(&fac, &bob, USDC).unwrap();
        assert!(
            env.advance(&fac, &bob, USDC).is_err(),
            "{tp:?} one live advance per borrower"
        );

        assert_eq!(
            env.open_facility_as(&rogue, 2, 100).unwrap_err(),
            err(RailsError::NotOperator)
        );
        let op = env.authority.insecure_clone();
        assert_eq!(
            env.open_facility_as(&op, 2, MAX_RATE_BPS + 1).unwrap_err(),
            err(RailsError::RateTooHigh)
        );
    });
}

// ── QC-P4 sweep order ───────────────────────────────────────────────────

#[test]
fn sweep_repays_facility_first_then_holder() {
    each(|tp| {
        let (mut env, fac, _) = with_facility(tp);
        let holder = env.party(0, 0);
        env.register(&holder, FAR).unwrap();
        let h = holder.key.pubkey();
        env.advance(&fac, &holder, 1_000 * USDC).unwrap();
        env.set_time(T0 + YEAR / 2); // 60 USDC interest
        let owed = 1_060 * USDC;

        // Claim sells for 1,500: facility takes exactly what's owed first.
        let proceeds = env.party(1_500 * USDC, 0);
        let vault = facility_vault_pda(&fac);
        let (v0, h0) = (env.bal(&vault), env.bal(&holder.usdc));
        env.sweep(
            &fac,
            &h,
            &proceeds,
            &dest_pda(&h),
            &holder.usdc,
            1_500 * USDC,
        )
        .unwrap();
        assert_eq!(
            env.bal(&vault) - v0,
            owed,
            "{tp:?} facility first, exactly owed"
        );
        assert_eq!(
            env.bal(&holder.usdc) - h0,
            1_500 * USDC - owed,
            "{tp:?} remainder to holder"
        );
        assert_eq!(env.bal(&proceeds.usdc), 0, "{tp:?} legs sum to proceeds");
        assert!(!env.exists(&advance_pda(&fac, &h)), "{tp:?} advance closed");
    });
}

#[test]
fn sweep_short_proceeds_all_to_facility_and_revoked_holder_blocks_remainder() {
    each(|tp| {
        let (mut env, fac, _) = with_facility(tp);
        let holder = env.party(0, 0);
        env.register(&holder, FAR).unwrap();
        let h = holder.key.pubkey();
        env.advance(&fac, &holder, 1_000 * USDC).unwrap();
        let hd = dest_pda(&h);

        // Proceeds below what's owed: all to the facility, holder gets 0,
        // advance stays open with principal reduced.
        let p1 = env.party(400 * USDC, 0);
        env.sweep(&fac, &h, &p1, &hd, &holder.usdc, 400 * USDC)
            .unwrap();
        assert_eq!(env.advance_state(&fac, &h).principal, 600 * USDC, "{tp:?}");
        assert_eq!(
            env.bal(&holder.usdc),
            1_000 * USDC,
            "{tp:?} holder untouched"
        );

        // Holder revoked: a sweep with a remainder is refused outright…
        env.revoke(&h).unwrap();
        let p2 = env.party(1_000 * USDC, 0);
        assert_eq!(
            env.sweep(&fac, &h, &p2, &hd, &holder.usdc, 1_000 * USDC)
                .unwrap_err(),
            err(RailsError::DestinationRevoked),
            "{tp:?}"
        );
        assert_eq!(env.bal(&p2.usdc), 1_000 * USDC, "{tp:?} nothing moved");
        // …but repaying the facility needs no payout, so it still works.
        env.sweep(&fac, &h, &p2, &hd, &holder.usdc, 600 * USDC)
            .unwrap();
        assert!(!env.exists(&advance_pda(&fac, &h)), "{tp:?} repaid in full");
        // Remainder can't be diverted to a non-holder account either.
        let thief = env.party(0, 0);
        env.renew(&h, &holder.usdc, FAR).unwrap();
        env.advance(&fac, &holder, 100 * USDC).unwrap();
        let p3 = env.party(500 * USDC, 0);
        assert_eq!(
            env.sweep(&fac, &h, &p3, &hd, &thief.usdc, 500 * USDC)
                .unwrap_err(),
            err(RailsError::DestinationNotAllowlisted)
        );
    });
}

// ── exit criterion 2: waterfall sums exactly to cash in ─────────────────

fn params() -> CouponParams {
    CouponParams {
        fees_due: 12 * USDC,
        senior_coupon_due: 300 * USDC,
        reserve_target: 100 * USDC,
        reserve_current: 40 * USDC,
        senior_amort_due: 150 * USDC,
        junior_coupon_due: 80 * USDC,
    }
}

#[test]
fn coupon_waterfall_sums_exactly_to_cash_in() {
    each(|tp| {
        let mut env = Env::new(tp);
        let senior = env.party(0, 0);
        let junior = env.party(0, 0);
        env.register(&senior, FAR).unwrap();
        env.register(&junior, FAR).unwrap();
        let auth = env.authority.pubkey();
        let mint = env.mint;
        let source = env.token_account(&mint, &auth, 10_000 * USDC);
        let sd = dest_pda(&senior.key.pubkey());
        let jd = dest_pda(&junior.key.pubkey());
        let all = [source, env.fee, env.reserve, senior.usdc, junior.usdc];
        let t0 = env.total(&all);

        // Full payment, shortfall, exactly-at-line-boundaries, odd amounts.
        for (period, cash) in (0u64..).zip([
            1_000 * USDC,
            200 * USDC,
            312 * USDC,
            372 * USDC,
            1_234_567,
            1,
        ]) {
            let before: Vec<u64> = all.iter().map(|a| env.bal(a)).collect();
            env.distribute(
                &source,
                period,
                cash,
                params(),
                (&sd, &senior.usdc),
                (&jd, &junior.usdc),
            )
            .unwrap();
            let want = CouponSplit::compute(cash, &params()).unwrap();
            let got: Vec<u64> = all
                .iter()
                .zip(&before)
                .map(|(a, b)| env.bal(a).abs_diff(*b))
                .collect();
            assert_eq!(got[0], cash, "{tp:?} source paid exactly cash");
            assert_eq!(
                (got[1], got[2], got[3], got[4]),
                (want.fees, want.reserve, want.senior, want.junior),
                "{tp:?} cash={cash} lines"
            );
            assert_eq!(
                got[1] + got[2] + got[3] + got[4],
                cash,
                "{tp:?} lines sum to cash in"
            );
            let r: payment_rails::CouponReceipt = env.read(&coupon_pda(&auth, period));
            assert_eq!(r.split, want);
            assert_eq!(env.total(&all), t0, "{tp:?} conserved");
        }
        // Replaying a period pays nothing.
        let s0 = env.bal(&source);
        assert!(env
            .distribute(
                &source,
                0,
                USDC,
                params(),
                (&sd, &senior.usdc),
                (&jd, &junior.usdc)
            )
            .is_err());
        assert_eq!(env.bal(&source), s0, "{tp:?} replay moved nothing");
    });
}

#[test]
fn coupon_holders_must_be_allowlisted() {
    each(|tp| {
        let mut env = Env::new(tp);
        let senior = env.party(0, 0);
        let junior = env.party(0, 0);
        env.register(&senior, FAR).unwrap();
        let auth = env.authority.pubkey();
        let mint = env.mint;
        let source = env.token_account(&mint, &auth, 10_000 * USDC);
        let sd = dest_pda(&senior.key.pubkey());
        let jd = dest_pda(&junior.key.pubkey());

        // Junior never registered → typed rejection, nothing paid to anyone.
        assert_eq!(
            env.distribute(
                &source,
                1,
                1_000 * USDC,
                params(),
                (&sd, &senior.usdc),
                (&jd, &junior.usdc)
            )
            .unwrap_err(),
            err(RailsError::DestinationNotAllowlisted),
            "{tp:?}"
        );
        assert_eq!(env.bal(&senior.usdc), 0, "{tp:?} atomic");
        // Cash that never reaches the junior lines doesn't need a junior.
        env.distribute(
            &source,
            2,
            200 * USDC,
            params(),
            (&sd, &senior.usdc),
            (&jd, &junior.usdc),
        )
        .unwrap();
        // Senior revoked → typed rejection.
        env.register(&junior, FAR).unwrap();
        env.revoke(&senior.key.pubkey()).unwrap();
        assert_eq!(
            env.distribute(
                &source,
                3,
                1_000 * USDC,
                params(),
                (&sd, &senior.usdc),
                (&jd, &junior.usdc)
            )
            .unwrap_err(),
            err(RailsError::DestinationRevoked)
        );
        // Swapping in the senior's registry entry for the junior's account.
        env.renew(&senior.key.pubkey(), &senior.usdc, FAR).unwrap();
        assert_eq!(
            env.distribute(
                &source,
                4,
                1_000 * USDC,
                params(),
                (&sd, &senior.usdc),
                (&sd, &junior.usdc)
            )
            .unwrap_err(),
            err(RailsError::DestinationNotAllowlisted)
        );
    });
}
