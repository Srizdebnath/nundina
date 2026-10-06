//! tranche-engine (#140, SRS 08 §4.6, AC-10) against the real
//! `tranche_engine.so`, `payment_rails.so` and `mark_engine.so`: a pool
//! exists only with its indenture hash; NPCS collateral mints senior and
//! junior at the attachment point; one coupon period's actual cash runs
//! the waterfall through payment-rails (fees → senior coupon → reserve →
//! junior coupon → junior residual, conserving every micro-USDC); the
//! subordination invariant blocks a junior exit; a mark-down impairs the
//! pool without touching principal; realised losses hit junior → reserve →
//! senior and recoveries reverse senior first.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{
    AccountDeserialize, AnchorDeserialize, Discriminator, InstructionData, ToAccountMetas,
};
use base64::Engine;
use common::*;
use payment_rails::{accounts as racc, CouponReceipt, EngineGrant};
use solana_keypair::Keypair;
use solana_signer::Signer;
use tranche_engine::{
    accounts as tacc, instruction as tix, PeriodDistributed, Pool, PoolParams, PoolState, TeError,
};

const UNIT: u64 = 1_000_000;
const SYSTEM: Pubkey = anchor_lang::system_program::ID;
const DOC: [u8; 32] = [0x1d; 32];

fn te(e: TeError) -> Fail {
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

fn tpda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &tranche_engine::ID).0
}

struct Fx {
    env: Env,
    pool: Pubkey,
    spv: Party,
    senior_acct: Pubkey,
    junior_acct: Pubkey,
    senior_holder: Party,
    junior_holder: Party,
    treasury: Party,
}

fn pool_of(env: &Env, pool: &Pubkey) -> Pool {
    let a = env.svm.get_account(pool).unwrap();
    Pool::try_deserialize(&mut &a.data[..]).unwrap()
}

fn create_ix(env: &Env, id: u32, params: PoolParams) -> Instruction {
    let pool = tpda(&[tranche_engine::POOL_SEED, &id.to_le_bytes()]);
    Instruction {
        program_id: tranche_engine::ID,
        accounts: tacc::CreatePool {
            pool,
            feed: feed_pda(&env.asset_mint),
            collateral_mint: env.asset_mint,
            senior_mint: tpda(&[tranche_engine::SENIOR_SEED, pool.as_ref()]),
            junior_mint: tpda(&[tranche_engine::JUNIOR_SEED, pool.as_ref()]),
            collateral_vault: tpda(&[tranche_engine::COLLATERAL_SEED, pool.as_ref()]),
            cash: tpda(&[tranche_engine::CASH_SEED, pool.as_ref()]),
            usdc_mint: env.mint,
            signer: tranche_engine::signer_pda().0,
            authority: env.authority.pubkey(),
            tranche_token_program: Tp::T22.id(),
            collateral_token_program: env.asset_tp.id(),
            token_program: env.tp.id(),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: tix::CreatePool {
            pool_id: id,
            params,
        }
        .data(),
    }
}

fn admin(env: &mut Env, pool: &Pubkey, data: Vec<u8>) -> Res {
    let a = env.authority.insecure_clone();
    let ix = Instruction {
        program_id: tranche_engine::ID,
        accounts: tacc::PoolAdmin {
            pool: *pool,
            authority: a.pubkey(),
        }
        .to_account_metas(None),
        data,
    };
    env.send(&[ix], &[&a])
}

/// Pool 1: 20% attachment, senior 6% / junior 12% / fees 0.1% APR,
/// reserve target 1 USDC; 100 NPCS at 1.00 → 80 senior + 20 junior.
fn fx() -> Fx {
    let mut env = Env::new(Tp::Spl);
    env.load_mark_engine();
    load_root_program(&mut env, tranche_engine::ID, "tranche_engine");
    let mint = env.asset_mint;
    let publisher = env.funded();
    env.init_feed(&mint, feed_params(), &publisher.pubkey());
    env.publish_nav(&mint, &publisher, NAV_ONE).unwrap();
    env.grant_engine(&tranche_engine::signer_pda().0, EngineGrant::ROLE_COUPON)
        .unwrap();
    let senior_holder = env.party(0, 0);
    let junior_holder = env.party(0, 0);
    env.register(&senior_holder, T0 + 400 * 86_400).unwrap();
    env.register(&junior_holder, T0 + 400 * 86_400).unwrap();
    let spv = env.party(0, 100 * UNIT);
    let treasury = env.party(100 * UNIT, 0);
    let mut params = PoolParams {
        legal_doc_hash: [0; 32],
        waterfall_version: 1,
        attachment_bps: 2_000,
        senior_coupon_bps: 600,
        junior_coupon_bps: 1_200,
        fees_bps: 10,
        reserve_target: UNIT,
        senior_payee_token: senior_holder.usdc,
        junior_payee_token: junior_holder.usdc,
    };
    // TE-P1: no indenture hash, no pool (and so no mint path).
    let a = env.authority.insecure_clone();
    let ix = create_ix(&env, 1, params);
    assert_eq!(env.send(&[ix], &[&a]).unwrap_err(), te(TeError::MissingDoc));
    params.legal_doc_hash = DOC;
    let ix = create_ix(&env, 1, params);
    env.send(&[ix], &[&a]).expect("create pool");
    let pool = tpda(&[tranche_engine::POOL_SEED, &1u32.to_le_bytes()]);
    let p = pool_of(&env, &pool);
    let senior_acct = env.token_account_in(Tp::T22, &p.senior_mint, &spv.key.pubkey(), 0);
    let junior_acct = env.token_account_in(Tp::T22, &p.junior_mint, &spv.key.pubkey(), 0);
    let ix = Instruction {
        program_id: tranche_engine::ID,
        accounts: tacc::Deposit {
            pool,
            feed: feed_pda(&mint),
            senior_mint: p.senior_mint,
            junior_mint: p.junior_mint,
            collateral_vault: p.collateral_vault,
            collateral_mint: mint,
            depositor: spv.key.pubkey(),
            depositor_collateral: spv.asset,
            depositor_senior: senior_acct,
            depositor_junior: junior_acct,
            signer: tranche_engine::signer_pda().0,
            tranche_token_program: Tp::T22.id(),
            collateral_token_program: env.asset_tp.id(),
        }
        .to_account_metas(None),
        data: tix::Deposit { qty: 100 * UNIT }.data(),
    };
    env.send(&[ix], &[&spv.key]).expect("deposit");
    admin(&mut env, &pool, tix::Activate {}.data()).expect("activate");
    Fx {
        env,
        pool,
        spv,
        senior_acct,
        junior_acct,
        senior_holder,
        junior_holder,
        treasury,
    }
}

fn distribute(f: &mut Fx, period: u32, cash: u64) -> Res {
    let p = pool_of(&f.env, &f.pool);
    let signer = tranche_engine::signer_pda().0;
    let mut ix = Instruction {
        program_id: tranche_engine::ID,
        accounts: tacc::Distribute {
            pool: f.pool,
            cash: p.cash,
            payer: f.treasury.key.pubkey(),
            payer_usdc: f.treasury.usdc,
            usdc_mint: f.env.mint,
            signer,
            token_program: f.env.tp.id(),
            rails_program: payment_rails::ID,
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: tix::Distribute { period, cash }.data(),
    };
    let rails: Vec<AccountMeta> = racc::DistributeCoupon {
        config: f.env.config,
        receipt: coupon_pda(&signer, tranche_engine::rails_period_id(1, period)),
        source: p.cash,
        fee_receiver: f.env.fee,
        reserve: f.env.reserve,
        senior_destination: dest_pda(&f.senior_holder.key.pubkey()),
        senior_token: f.senior_holder.usdc,
        junior_destination: dest_pda(&f.junior_holder.key.pubkey()),
        junior_token: f.junior_holder.usdc,
        usdc_mint: f.env.mint,
        operator: signer,
        token_program: f.env.tp.id(),
        system_program: SYSTEM,
    }
    .to_account_metas(None)
    .into_iter()
    .map(|m| AccountMeta {
        is_signer: m.is_signer && m.pubkey != signer,
        ..m
    })
    .chain([AccountMeta::new_readonly(grant_pda(&signer), false)])
    .collect();
    ix.accounts.extend(rails);
    let k = f.treasury.key.insecure_clone();
    f.env.send(&[ix], &[&k])
}

fn exit_junior(f: &mut Fx, amount: u64) -> Res {
    let p = pool_of(&f.env, &f.pool);
    let ix = Instruction {
        program_id: tranche_engine::ID,
        accounts: tacc::ExitJunior {
            pool: f.pool,
            feed: feed_pda(&f.env.asset_mint),
            junior_mint: p.junior_mint,
            collateral_vault: p.collateral_vault,
            collateral_mint: f.env.asset_mint,
            holder: f.spv.key.pubkey(),
            holder_junior: f.junior_acct,
            holder_collateral: f.spv.asset,
            signer: tranche_engine::signer_pda().0,
            tranche_token_program: Tp::T22.id(),
            collateral_token_program: f.env.asset_tp.id(),
        }
        .to_account_metas(None),
        data: tix::ExitJunior { amount }.data(),
    };
    let k = f.spv.key.insecure_clone();
    f.env.send(&[ix], &[&k])
}

fn mark(f: &mut Fx, caller: &Keypair) -> Res {
    let ix = Instruction {
        program_id: tranche_engine::ID,
        accounts: tacc::Mark {
            pool: f.pool,
            feed: feed_pda(&f.env.asset_mint),
        }
        .to_account_metas(None),
        data: tix::Mark {}.data(),
    };
    f.env.send(&[ix], &[caller])
}

#[test]
fn ac10_deposit_mints_at_the_attachment_and_junior_is_locked() {
    let f = fx();
    assert_eq!(f.env.bal(&f.senior_acct), 80 * UNIT);
    assert_eq!(f.env.bal(&f.junior_acct), 20 * UNIT);
    let p = pool_of(&f.env, &f.pool);
    assert_eq!(
        (p.senior_principal, p.junior_principal, p.state),
        (80 * UNIT, 20 * UNIT, PoolState::Active)
    );
    let a = f.env.svm.get_account(&f.junior_acct).unwrap();
    assert_eq!(a.data[108], 2, "junior account frozen (lock-up)");
    let a = f.env.svm.get_account(&f.senior_acct).unwrap();
    assert_eq!(a.data[108], 1, "senior transferable");
}

#[test]
fn ac10_one_coupon_period_runs_the_waterfall_through_the_rails() {
    let mut f = fx();
    let now = f.env.now();
    f.env.set_time(now + 30 * 86_400);
    // Periods run in order.
    assert_eq!(
        distribute(&mut f, 1, 2 * UNIT).unwrap_err(),
        te(TeError::WrongPeriod)
    );
    let fee0 = f.env.bal(&f.env.fee);
    let res0 = f.env.bal(&f.env.reserve);
    let meta = distribute(&mut f, 0, 2 * UNIT).unwrap();
    let d = &events::<PeriodDistributed>(&meta.logs)[0];
    // 30 days: senior 80 × 6% → 394_520; junior 20 × 12% → 197_260;
    // fees 100 × 0.1% → 8_219; reserve to 1 USDC; residual to junior.
    assert_eq!(d.senior_coupon, 394_520);
    assert_eq!(d.junior_coupon, 197_260);
    assert_eq!(d.fees, 8_219);
    assert_eq!(d.reserve, UNIT);
    assert_eq!(d.senior_amort, 0);
    assert_eq!(
        d.fees + d.senior_coupon + d.reserve + d.junior_coupon + d.junior_residual,
        2 * UNIT,
        "every micro-USDC distributed"
    );
    // The rails moved exactly those lines.
    assert_eq!(f.env.bal(&f.senior_holder.usdc), d.senior_coupon);
    assert_eq!(
        f.env.bal(&f.junior_holder.usdc),
        d.junior_coupon + d.junior_residual
    );
    assert_eq!(f.env.bal(&f.env.fee) - fee0, d.fees);
    assert_eq!(f.env.bal(&f.env.reserve) - res0, d.reserve);
    let signer = tranche_engine::signer_pda().0;
    let r: CouponReceipt = f
        .env
        .read(&coupon_pda(&signer, tranche_engine::rails_period_id(1, 0)));
    assert_eq!(r.split.cash, 2 * UNIT);
    assert_eq!(pool_of(&f.env, &f.pool).reserve_level, UNIT);
    // The same period cannot be paid twice.
    assert!(distribute(&mut f, 0, 2 * UNIT).is_err());
}

#[test]
fn ac10_subordination_invariant_blocks_a_junior_exit() {
    let mut f = fx();
    // Any junior exit from an exactly-20% pool would take it below 20%.
    assert_eq!(
        exit_junior(&mut f, UNIT).unwrap_err(),
        te(TeError::SubordinationBreach)
    );
    // A pool marked up to 1.10 has headroom: 110 value, 80 senior.
    let mint = f.env.asset_mint;
    let now = f.env.now();
    f.env.set_time(now + 60);
    f.env.drill_nav(&mint, NAV_ONE * 11 / 10).unwrap();
    exit_junior(&mut f, UNIT).unwrap();
    let p = pool_of(&f.env, &f.pool);
    assert_eq!(p.junior_principal, 19 * UNIT);
    assert_eq!(f.env.bal(&f.junior_acct), 19 * UNIT);
}

#[test]
fn ac10_three_books_mark_loss_recovery() {
    let mut f = fx();
    let crank = f.env.funded();
    // MARK book: 0.85 → −15 USDC unrealised, principal untouched, and the
    // 20% junior no longer covers it: Impaired.
    let mint = f.env.asset_mint;
    let now = f.env.now();
    f.env.set_time(now + 60);
    f.env.drill_nav(&mint, NAV_ONE * 85 / 100).unwrap();
    mark(&mut f, &crank).unwrap();
    let p = pool_of(&f.env, &f.pool);
    assert_eq!(p.unrealised_mark, -15 * UNIT as i64);
    assert_eq!(
        (p.senior_principal, p.junior_principal),
        (80 * UNIT, 20 * UNIT)
    );
    assert_eq!(p.state, PoolState::Impaired);
    // TE-P7: override back (authority + document hash).
    assert_eq!(
        admin(
            &mut f.env,
            &f.pool,
            tix::HumanOverride {
                to: PoolState::Active,
                authority_doc_hash: [0; 32],
            }
            .data()
        )
        .unwrap_err(),
        te(TeError::MissingDoc)
    );
    admin(
        &mut f.env,
        &f.pool,
        tix::HumanOverride {
            to: PoolState::Active,
            authority_doc_hash: [0xa7; 32],
        }
        .data(),
    )
    .unwrap();
    // REALISED book: 25 USDC loss → junior 20, senior 5 (no reserve yet).
    admin(
        &mut f.env,
        &f.pool,
        tix::RealiseLoss {
            amount: 25 * UNIT,
            ref_hash: [3; 32],
        }
        .data(),
    )
    .unwrap();
    let p = pool_of(&f.env, &f.pool);
    assert_eq!((p.junior_principal, p.senior_principal), (0, 75 * UNIT));
    assert_eq!(
        (p.realised_loss, p.recovery_receivable),
        (25 * UNIT, 25 * UNIT)
    );
    assert_eq!(p.state, PoolState::Impaired);
    // RECOVERY book: 8 USDC restores senior first (5), then junior (3).
    admin(
        &mut f.env,
        &f.pool,
        tix::RecordRecovery {
            amount: 8 * UNIT,
            ref_hash: [4; 32],
        }
        .data(),
    )
    .unwrap();
    let p = pool_of(&f.env, &f.pool);
    assert_eq!(
        (p.senior_principal, p.junior_principal),
        (80 * UNIT, 3 * UNIT)
    );
    assert_eq!(p.recovery_receivable, 17 * UNIT);
    assert_eq!(
        admin(
            &mut f.env,
            &f.pool,
            tix::RecordRecovery {
                amount: 18 * UNIT,
                ref_hash: [4; 32],
            }
            .data()
        )
        .unwrap_err(),
        te(TeError::RecoveryAboveReceivable)
    );
}
