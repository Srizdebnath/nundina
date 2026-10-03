//! #127 / doc 06 §8: bonded keepers with a permissionless fallback.
//! A keeper bonds USDC, the operator assigns it to a session, and after
//! `settle_deadline + sla` anyone may refund what the keeper left open and
//! be paid from the keeper's bond. Liveness never depends on the bond.
//! Every test runs under SPL Token and Token-2022.

mod common;

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{InstructionData, ToAccountMetas};
use common::*;
use payment_rails::{
    accounts as acc, instruction as ix, Keeper, KeeperPolicy, RailsError, SessionKeeper,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

const MIN_BOND: u64 = 50_000_000; // 50 USDC
const REWARD: u64 = 20_000_000; // 20 USDC per escalated escrow
const SLA: i64 = 3_600;
const CASH: u64 = 100_000_000;

fn each(test: impl Fn(Tp)) {
    for tp in Tp::ALL {
        test(tp);
    }
}

fn err(e: RailsError) -> Fail {
    Fail::rails(e)
}

fn policy_pda() -> Pubkey {
    pda(&[KeeperPolicy::SEED])
}
fn keeper_pda(wallet: &Pubkey) -> Pubkey {
    pda(&[Keeper::SEED, wallet.as_ref()])
}
fn keeper_vault(wallet: &Pubkey) -> Pubkey {
    pda(&[Keeper::VAULT_SEED, keeper_pda(wallet).as_ref()])
}
fn session_keeper_pda(session: &Pubkey) -> Pubkey {
    pda(&[SessionKeeper::SEED, session.as_ref()])
}

use anchor_lang::prelude::Pubkey;

trait KeeperEnv {
    fn init_policy(&mut self, signer: &Keypair, min: u64, sla: i64, reward: u64) -> Res;
    fn register_keeper(&mut self, keeper: &Keypair, token: &Pubkey, bond: u64) -> Res;
    fn assign_keeper(&mut self, operator: &Keypair, session: &Pubkey, keeper: &Pubkey) -> Res;
    #[allow(clippy::too_many_arguments)]
    fn escalate(
        &mut self,
        caller: &Keypair,
        caller_token: &Pubkey,
        session: &Pubkey,
        sk_session: &Pubkey,
        owner: &Pubkey,
        owner_token: &Pubkey,
        keeper: &Pubkey,
    ) -> Res;
    fn escalate_ask(
        &mut self,
        caller: &Keypair,
        caller_token: &Pubkey,
        session: &Pubkey,
        who: &Party,
        keeper: &Pubkey,
    ) -> Res;
    fn release_duty(&mut self, caller: &Keypair, session: &Pubkey, keeper: &Pubkey) -> Res;
    fn withdraw_bond(&mut self, keeper: &Keypair, token: &Pubkey) -> Res;
    fn keeper_state(&self, wallet: &Pubkey) -> Keeper;
}

impl KeeperEnv for Env {
    fn init_policy(&mut self, signer: &Keypair, min: u64, sla: i64, reward: u64) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeKeeperPolicy {
                config: self.config,
                policy: policy_pda(),
                authority: signer.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::InitializeKeeperPolicy {
                min_bond: min,
                sla_seconds: sla,
                escalation_reward: reward,
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    fn register_keeper(&mut self, keeper: &Keypair, token: &Pubkey, bond: u64) -> Res {
        let w = keeper.pubkey();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::RegisterKeeper {
                config: self.config,
                policy: policy_pda(),
                keeper: keeper_pda(&w),
                vault: keeper_vault(&w),
                usdc_mint: self.mint,
                keeper_token: *token,
                wallet: w,
                token_program: self.tp.id(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::RegisterKeeper { bond }.data(),
        };
        self.send(&[ix], &[keeper])
    }

    fn assign_keeper(&mut self, operator: &Keypair, session: &Pubkey, keeper: &Pubkey) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AssignKeeper {
                policy: policy_pda(),
                session: *session,
                keeper: keeper_pda(keeper),
                vault: keeper_vault(keeper),
                session_keeper: session_keeper_pda(session),
                operator: operator.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::AssignKeeper {}.data(),
        };
        self.send(&[ix], &[operator])
    }

    #[allow(clippy::too_many_arguments)]
    fn escalate(
        &mut self,
        caller: &Keypair,
        caller_token: &Pubkey,
        session: &Pubkey,
        sk_session: &Pubkey,
        owner: &Pubkey,
        owner_token: &Pubkey,
        keeper: &Pubkey,
    ) -> Res {
        let escrow = escrow_pda(session, owner);
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::RefundEscalated {
                refund: acc::Refund {
                    caller: caller.pubkey(),
                    session: *session,
                    escrow,
                    vault: vault_pda(&escrow),
                    owner: *owner,
                    owner_token: *owner_token,
                    reserve: self.reserve,
                    usdc_mint: self.mint,
                    token_program: self.tp.id(),
                },
                keeper: acc::KeeperAccounts {
                    session_keeper: session_keeper_pda(sk_session),
                    keeper: keeper_pda(keeper),
                    vault: keeper_vault(keeper),
                    caller_token: *caller_token,
                },
            }
            .to_account_metas(None),
            data: ix::RefundEscalated {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    fn escalate_ask(
        &mut self,
        caller: &Keypair,
        caller_token: &Pubkey,
        session: &Pubkey,
        who: &Party,
        keeper: &Pubkey,
    ) -> Res {
        let escrow = escrow_pda(session, &who.key.pubkey());
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::RefundAskEscalated {
                refund: acc::RefundAsk {
                    caller: caller.pubkey(),
                    session: *session,
                    escrow,
                    vault: vault_pda(&escrow),
                    asset_vault: asset_vault_pda(&escrow),
                    owner: who.key.pubkey(),
                    owner_usdc: who.usdc,
                    owner_asset: who.asset,
                    reserve: self.reserve,
                    usdc_mint: self.mint,
                    asset_mint: self.asset_mint,
                    token_program: self.tp.id(),
                    asset_token_program: self.asset_tp.id(),
                },
                keeper: acc::KeeperAccounts {
                    session_keeper: session_keeper_pda(session),
                    keeper: keeper_pda(keeper),
                    vault: keeper_vault(keeper),
                    caller_token: *caller_token,
                },
            }
            .to_account_metas(None),
            data: ix::RefundAskEscalated {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    fn release_duty(&mut self, caller: &Keypair, session: &Pubkey, keeper: &Pubkey) -> Res {
        let operator = self.session(session).operator;
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::ReleaseAssignment {
                session: *session,
                session_keeper: session_keeper_pda(session),
                keeper: keeper_pda(keeper),
                operator,
                caller: caller.pubkey(),
            }
            .to_account_metas(None),
            data: ix::ReleaseAssignment {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    fn withdraw_bond(&mut self, keeper: &Keypair, token: &Pubkey) -> Res {
        let w = keeper.pubkey();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::WithdrawKeeper {
                keeper: keeper_pda(&w),
                vault: keeper_vault(&w),
                usdc_mint: self.mint,
                keeper_token: *token,
                wallet: w,
                token_program: self.tp.id(),
            }
            .to_account_metas(None),
            data: ix::WithdrawKeeper {}.data(),
        };
        self.send(&[ix], &[keeper])
    }

    fn keeper_state(&self, wallet: &Pubkey) -> Keeper {
        self.read(&keeper_pda(wallet))
    }
}

/// Policy + a bonded keeper + one session with two unrevealed buy escrows,
/// keeper assigned. Returns (keeper, keeper_token, session, bidders).
fn setup(tp: Tp, bond: u64) -> (Env, Keypair, Pubkey, Pubkey, Vec<(Keypair, Pubkey)>) {
    let mut env = Env::new(tp);
    let auth = env.authority.insecure_clone();
    env.init_policy(&auth, MIN_BOND, SLA, REWARD)
        .expect("policy");
    let (k, kt) = env.bidder(500_000_000);
    env.register_keeper(&k, &kt, bond).expect("register");
    let s = env.open_session(1);
    let bidders: Vec<_> = (0..2)
        .map(|_| {
            let (b, bt) = env.bidder(CASH + BOND);
            env.deposit(&s, &b, &bt, CASH).expect("deposit");
            (b, bt)
        })
        .collect();
    env.assign_keeper(&auth, &s, &k.pubkey()).expect("assign");
    (env, k, kt, s, bidders)
}

#[test]
fn policy_is_bounded_and_authority_only() {
    each(|tp| {
        let mut env = Env::new(tp);
        let auth = env.authority.insecure_clone();
        let stranger = env.funded();
        let oob = err(RailsError::KeeperPolicyOutOfBounds);
        assert_eq!(
            env.init_policy(&auth, MIN_BOND, 0, REWARD).unwrap_err(),
            oob,
            "zero SLA"
        );
        assert_eq!(
            env.init_policy(&auth, MIN_BOND, 31 * 86_400, REWARD)
                .unwrap_err(),
            oob
        );
        assert_eq!(
            env.init_policy(&auth, MIN_BOND, SLA, 0).unwrap_err(),
            oob,
            "zero reward"
        );
        assert_eq!(
            env.init_policy(&auth, MIN_BOND, SLA, MIN_BOND + 1)
                .unwrap_err(),
            oob,
            "reward > bond"
        );
        assert!(env.init_policy(&stranger, MIN_BOND, SLA, REWARD).is_err());
        env.init_policy(&auth, MIN_BOND, SLA, REWARD).unwrap();
    });
}

#[test]
fn bond_below_minimum_is_refused_and_assignment_is_operator_only() {
    each(|tp| {
        let mut env = Env::new(tp);
        let auth = env.authority.insecure_clone();
        env.init_policy(&auth, MIN_BOND, SLA, REWARD).unwrap();
        let (k, kt) = env.bidder(500_000_000);
        assert_eq!(
            env.register_keeper(&k, &kt, MIN_BOND - 1).unwrap_err(),
            err(RailsError::KeeperBondTooLow)
        );
        env.register_keeper(&k, &kt, MIN_BOND).unwrap();
        assert_eq!(
            env.bal(&keeper_vault(&k.pubkey())),
            MIN_BOND,
            "bond held by the keeper PDA's vault"
        );
        let s = env.open_session(1);
        let mallory = env.funded();
        assert!(
            env.assign_keeper(&mallory, &s, &k.pubkey()).is_err(),
            "only the session operator assigns"
        );
        env.assign_keeper(&auth, &s, &k.pubkey()).unwrap();
        assert!(
            env.assign_keeper(&auth, &s, &k.pubkey()).is_err(),
            "one keeper per session"
        );
        assert_eq!(env.keeper_state(&k.pubkey()).assignments, 1);
    });
}

#[test]
fn escalation_waits_for_the_sla_then_pays_the_caller_from_the_bond() {
    each(|tp| {
        let (mut env, k, _kt, s, bidders) = setup(tp, MIN_BOND);
        let (stranger, st) = env.bidder(0);
        let (b0, b0t) = (&bidders[0].0.pubkey(), bidders[0].1);
        // Refundable after the hard deadline, but the keeper's SLA runs on.
        env.set_time(SETTLE_DEADLINE + SLA - 1);
        assert_eq!(
            env.escalate(&stranger, &st, &s, &s, b0, &b0t, &k.pubkey())
                .unwrap_err(),
            err(RailsError::KeeperSlaNotDue)
        );
        // The keeper can (and should) refund normally inside the SLA.
        let kk = k.insecure_clone();
        env.refund(&kk, &s, b0, &b0t).unwrap();
        assert_eq!(
            env.bal(&b0t),
            CASH,
            "unrevealed bond forfeit, cash back (AE-P4)"
        );

        // Past the SLA the second escrow is still open: anyone escalates.
        env.set_time(SETTLE_DEADLINE + SLA);
        let (b1, b1t) = (bidders[1].0.pubkey(), bidders[1].1);
        env.escalate(&stranger, &st, &s, &s, &b1, &b1t, &k.pubkey())
            .unwrap();
        assert_eq!(env.bal(&b1t), CASH, "same refund as `refund`");
        assert!(!env.exists(&escrow_pda(&s, &b1)), "escrow closed");
        assert_eq!(env.bal(&st), REWARD, "caller paid from the bond");
        assert_eq!(env.bal(&keeper_vault(&k.pubkey())), MIN_BOND - REWARD);
        assert_eq!(env.keeper_state(&k.pubkey()).missed, 1);
        let sk: SessionKeeper = env.read(&session_keeper_pda(&s));
        assert_eq!(sk.escalations, 1);
        assert_eq!(env.session(&s).open_escrows, 0);
    });
}

#[test]
fn an_exhausted_bond_never_blocks_a_refund() {
    each(|tp| {
        // Bond covers 2.5 rewards: 20 + 20 + 10 + 0.
        let mut env = Env::new(tp);
        let auth = env.authority.insecure_clone();
        env.init_policy(&auth, MIN_BOND, SLA, REWARD).unwrap();
        let (k, kt) = env.bidder(500_000_000);
        env.register_keeper(&k, &kt, MIN_BOND).unwrap();
        let s = env.open_session(1);
        let bidders: Vec<_> = (0..4)
            .map(|_| {
                let (b, bt) = env.bidder(CASH + BOND);
                env.deposit(&s, &b, &bt, CASH).unwrap();
                (b, bt)
            })
            .collect();
        env.assign_keeper(&auth, &s, &k.pubkey()).unwrap();
        // Opened now: a session can't be opened with a schedule in the past.
        let s2 = env.open_session(2);
        env.set_time(SETTLE_DEADLINE + SLA);
        let (stranger, st) = env.bidder(0);
        let mut paid = vec![];
        for (b, bt) in &bidders {
            let before = env.bal(&st);
            env.escalate(&stranger, &st, &s, &s, &b.pubkey(), bt, &k.pubkey())
                .unwrap();
            assert_eq!(env.bal(bt), CASH, "every escrow refunded");
            paid.push(env.bal(&st) - before);
        }
        assert_eq!(paid, vec![REWARD, REWARD, MIN_BOND - 2 * REWARD, 0]);
        assert_eq!(env.bal(&keeper_vault(&k.pubkey())), 0);
        assert_eq!(env.keeper_state(&k.pubkey()).missed, 4);
        // An empty bond can't be assigned again.
        assert_eq!(
            env.assign_keeper(&auth, &s2, &k.pubkey()).unwrap_err(),
            err(RailsError::KeeperBondTooLow)
        );
    });
}

#[test]
fn an_assignment_cannot_pay_out_for_another_session() {
    each(|tp| {
        let (mut env, k, _kt, s, _bidders) = setup(tp, MIN_BOND);
        // A second session without this keeper.
        let s2 = env.open_session(2);
        let (b, bt) = env.bidder(CASH + BOND);
        env.deposit(&s2, &b, &bt, CASH).unwrap();
        env.set_time(SETTLE_DEADLINE + SLA);
        let (stranger, st) = env.bidder(0);
        assert_eq!(
            env.escalate(&stranger, &st, &s2, &s, &b.pubkey(), &bt, &k.pubkey())
                .unwrap_err(),
            err(RailsError::KeeperSessionMismatch)
        );
        assert_eq!(env.bal(&st), 0);
    });
}

#[test]
fn the_bond_is_locked_until_the_duty_is_released() {
    each(|tp| {
        let (mut env, k, kt, s, bidders) = setup(tp, MIN_BOND);
        let before = env.bal(&kt);
        assert_eq!(
            env.withdraw_bond(&k, &kt).unwrap_err(),
            err(RailsError::KeeperAssigned)
        );
        let anyone = env.funded();
        assert_eq!(
            env.release_duty(&anyone, &s, &k.pubkey()).unwrap_err(),
            err(RailsError::SessionStillOpen)
        );
        env.set_time(SETTLE_DEADLINE);
        for (b, bt) in &bidders {
            let kk = k.insecure_clone();
            env.refund(&kk, &s, &b.pubkey(), bt).unwrap();
        }
        env.release_duty(&anyone, &s, &k.pubkey()).unwrap();
        assert!(!env.exists(&session_keeper_pda(&s)));
        assert_eq!(env.keeper_state(&k.pubkey()).assignments, 0);
        env.withdraw_bond(&k, &kt).unwrap();
        assert_eq!(env.bal(&kt), before + MIN_BOND, "whole bond back");
        assert!(!env.exists(&keeper_pda(&k.pubkey())) && !env.exists(&keeper_vault(&k.pubkey())));
    });
}

#[test]
fn sell_escrows_escalate_too() {
    each(|tp| {
        let mut env = Env::new(tp);
        let auth = env.authority.insecure_clone();
        env.init_policy(&auth, MIN_BOND, SLA, REWARD).unwrap();
        let (k, kt) = env.bidder(500_000_000);
        env.register_keeper(&k, &kt, MIN_BOND).unwrap();
        let s = env.open_session(1);
        let seller = env.party(BOND, 1_000_000);
        env.deposit_ask(&s, &seller, 1_000_000).unwrap();
        env.assign_keeper(&auth, &s, &k.pubkey()).unwrap();
        env.set_time(SETTLE_DEADLINE + SLA);
        let (stranger, st) = env.bidder(0);
        env.escalate_ask(&stranger, &st, &s, &seller, &k.pubkey())
            .unwrap();
        assert_eq!(env.bal(&seller.asset), 1_000_000, "asset returned");
        assert_eq!(env.bal(&st), REWARD);
    });
}

#[test]
fn the_owner_can_escalate_its_own_late_refund() {
    each(|tp| {
        let (mut env, k, _kt, s, bidders) = setup(tp, MIN_BOND);
        env.set_time(SETTLE_DEADLINE + SLA);
        let (owner, owner_token) = (&bidders[0].0, bidders[0].1);
        // Caller = owner, caller_token = owner_token: one account, twice.
        env.escalate(
            owner,
            &owner_token,
            &s,
            &s,
            &owner.pubkey(),
            &owner_token,
            &k.pubkey(),
        )
        .unwrap();
        assert_eq!(
            env.bal(&owner_token),
            CASH + REWARD,
            "cash back + reward for cranking it"
        );
        assert_eq!(env.keeper_state(&k.pubkey()).missed, 1);
    });
}
