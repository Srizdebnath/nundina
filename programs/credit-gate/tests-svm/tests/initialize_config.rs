//! #26 regression: the first-caller-wins `init` of a mint's `MarketConfig`
//! cannot be front-run into a permanent gate-authority takeover.
//! #93: configs are authorized by the gate root, not by `ProgramData`, so
//! market onboarding keeps working after the program is finalized (doc 06
//! §11 U1), while only the upgrade authority can seed the root.
//!
//! Loads the real SBF build (build.rs) as an upgradeable program.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use common::{
    deploy, init_root_ix, programdata_pda, root_pda, seed_root, set_upgrade_authority, PROGRAM_ID,
};
use credit_gate::{accounts as acc, instruction as ix, GateError, GateRoot, MarketConfig};
use litesvm::LiteSVM;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

/// anchor `ConstraintRaw` — a `constraint = …` without a custom error.
const CONSTRAINT_RAW: u32 = 2003;
/// anchor `AccountNotInitialized`.
const ACCOUNT_NOT_INITIALIZED: u32 = 3012;

#[derive(Debug, PartialEq, Eq)]
enum Fail {
    Code(u32),
    Other(String),
}

fn gate(e: GateError) -> Fail {
    Fail::Code(u32::from(e))
}

struct Env {
    svm: LiteSVM,
    /// Upgrade authority, and (after `Env::new`) the gate root authority.
    authority: Keypair,
}

impl Env {
    /// Program deployed with `authority` as upgrade authority; root NOT seeded.
    fn bare() -> Env {
        let authority = Keypair::new();
        let svm = deploy(&authority);
        Env { svm, authority }
    }

    /// Program deployed, root seeded with the upgrade authority as root.
    fn new() -> Env {
        let mut env = Env::bare();
        seed_root(&mut env.svm, &env.authority);
        env
    }

    fn funded(&mut self) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        k
    }

    fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<(), Fail> {
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&signers[0].pubkey()),
            signers,
            self.svm.latest_blockhash(),
        );
        let out = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        out.map(|_| ()).map_err(|f| match f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(c)) => Fail::Code(c),
            other => Fail::Other(format!("{other:?} | {:?}", f.meta.logs)),
        })
    }

    fn init_ix(&self, signer: &Pubkey, mint: &Pubkey) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeConfig {
                config: config_pda(mint),
                root: root_pda(),
                mint: *mint,
                authority: *signer,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::InitializeConfig {
                policy_class: 1,
                holder_cap: 8,
                seasoning_seconds: 7 * 86_400,
                settlement_horizon_seconds: 86_400,
                attesters: vec![Pubkey::new_unique()],
                identity_schema: Pubkey::new_unique(),
                accreditation_schema: Pubkey::new_unique(),
            }
            .data(),
        }
    }

    fn init_as(&mut self, signer: &Keypair, mint: &Pubkey) -> Result<(), Fail> {
        let ix = self.init_ix(&signer.pubkey(), mint);
        self.send(&[ix], &[signer])
    }

    fn seed_root_as(&mut self, signer: &Keypair, root_authority: Pubkey) -> Result<(), Fail> {
        let ix = init_root_ix(&signer.pubkey(), root_authority, programdata_pda());
        self.send(&[ix], &[signer])
    }

    fn govern_root(&mut self, signer: &Keypair, data: impl InstructionData) -> Result<(), Fail> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::GovernRoot {
                root: root_pda(),
                authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: data.data(),
        };
        self.send(&[ix], &[signer])
    }

    fn propose_root(&mut self, signer: &Keypair, new_authority: Pubkey) -> Result<(), Fail> {
        self.govern_root(signer, ix::ProposeRootAuthority { new_authority })
    }

    fn cancel_root(&mut self, signer: &Keypair) -> Result<(), Fail> {
        self.govern_root(signer, ix::CancelRootAuthority {})
    }

    fn accept_root(&mut self, signer: &Keypair) -> Result<(), Fail> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AcceptRootAuthority {
                root: root_pda(),
                new_authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix::AcceptRootAuthority {}.data(),
        };
        self.send(&[ix], &[signer])
    }

    fn update_holder_cap_as(
        &mut self,
        signer: &Keypair,
        mint: &Pubkey,
        cap: u32,
    ) -> Result<(), Fail> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::UpdateConfig {
                config: config_pda(mint),
                mint: *mint,
                gate_config_authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix::UpdateConfig {
                policy_class: None,
                holder_cap: Some(cap),
                seasoning_seconds: None,
                settlement_horizon_seconds: None,
                attester_add: None,
                attester_remove: None,
                blocklist_set: None,
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    fn config(&self, mint: &Pubkey) -> Option<MarketConfig> {
        let a = self.svm.get_account(&config_pda(mint))?;
        if a.data.is_empty() {
            return None;
        }
        Some(MarketConfig::try_deserialize(&mut a.data.as_slice()).expect("decode config"))
    }

    fn root(&self) -> Option<GateRoot> {
        let a = self.svm.get_account(&root_pda())?;
        if a.data.is_empty() {
            return None;
        }
        Some(GateRoot::try_deserialize(&mut a.data.as_slice()).expect("decode root"))
    }
}

fn config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[MarketConfig::SEED, mint.as_ref()], &PROGRAM_ID).0
}

#[test]
fn root_authority_initializes_and_governs() {
    let mut env = Env::new();
    let mint = Pubkey::new_unique();
    let auth = env.authority.insecure_clone();
    env.init_as(&auth, &mint)
        .expect("root authority initializes");

    let cfg = env.config(&mint).expect("config exists");
    assert_eq!(cfg.mint, mint);
    assert_eq!(cfg.gate_config_authority, auth.pubkey());
    assert_eq!(cfg.holder_cap, 8);

    env.update_holder_cap_as(&auth, &mint, 9)
        .expect("authority updates");
    assert_eq!(env.config(&mint).unwrap().holder_cap, 9);
}

#[test]
fn front_runner_cannot_initialize() {
    let mut env = Env::new();
    let mint = Pubkey::new_unique();
    let attacker = env.funded();

    assert_eq!(
        env.init_as(&attacker, &mint),
        Err(gate(GateError::NotRootAuthority))
    );
    assert!(
        env.config(&mint).is_none(),
        "rejected init must not create the PDA"
    );

    // The legitimate call still lands after the failed front-run.
    let auth = env.authority.insecure_clone();
    env.init_as(&auth, &mint)
        .expect("root authority initializes");
    assert_eq!(
        env.config(&mint).unwrap().gate_config_authority,
        auth.pubkey()
    );

    // …and the attacker holds no power over it.
    assert_eq!(
        env.update_holder_cap_as(&attacker, &mint, u32::MAX),
        Err(gate(GateError::Unauthorized))
    );
    assert_eq!(env.config(&mint).unwrap().holder_cap, 8);
}

#[test]
fn no_config_before_the_root_is_seeded() {
    let mut env = Env::bare();
    let auth = env.authority.insecure_clone();
    let mint = Pubkey::new_unique();
    assert_eq!(
        env.init_as(&auth, &mint),
        Err(Fail::Code(ACCOUNT_NOT_INITIALIZED)),
        "not even the upgrade authority creates configs without a root"
    );
}

#[test]
fn only_the_upgrade_authority_seeds_the_root_once() {
    let mut env = Env::bare();
    let attacker = env.funded();
    assert_eq!(
        env.seed_root_as(&attacker, attacker.pubkey()),
        Err(gate(GateError::NotUpgradeAuthority))
    );
    assert!(env.root().is_none());

    // A loader-owned ProgramData-shaped account naming the attacker as
    // upgrade authority, at an address that is not this program's.
    let mut fake = env.svm.get_account(&programdata_pda()).unwrap();
    fake.data[12] = 1;
    fake.data[13..45].copy_from_slice(attacker.pubkey().as_ref());
    let fake_key = Pubkey::new_unique();
    env.svm.set_account(fake_key, fake).unwrap();
    let ix = init_root_ix(&attacker.pubkey(), attacker.pubkey(), fake_key);
    assert_eq!(
        env.send(&[ix], &[&attacker]),
        Err(Fail::Code(CONSTRAINT_RAW))
    );
    assert!(env.root().is_none());

    let auth = env.authority.insecure_clone();
    assert_eq!(
        env.seed_root_as(&auth, Pubkey::default()),
        Err(gate(GateError::InvalidNewAuthority))
    );
    env.seed_root_as(&auth, auth.pubkey()).expect("seed");
    // One-shot: `init` refuses a second root (system code 0).
    assert_eq!(
        env.seed_root_as(&auth, attacker.pubkey()),
        Err(Fail::Code(0))
    );
    assert_eq!(env.root().unwrap().authority, auth.pubkey());
}

#[test]
fn finalized_program_still_onboards_markets() {
    let mut env = Env::new();
    let auth = env.authority.insecure_clone();
    env.init_as(&auth, &Pubkey::new_unique())
        .expect("before finalize");

    // `solana program finalize`: the upgrade authority becomes None.
    set_upgrade_authority(&mut env.svm, None);

    let mint = Pubkey::new_unique();
    env.init_as(&auth, &mint)
        .expect("a finalized program still creates per-mint configs (#93)");
    assert_eq!(
        env.config(&mint).unwrap().gate_config_authority,
        auth.pubkey()
    );
    // …and the root's own governance does not depend on ProgramData either.
    let next = env.funded();
    env.propose_root(&auth, next.pubkey()).expect("propose");
    env.accept_root(&next).expect("accept");
    env.init_as(&next, &Pubkey::new_unique())
        .expect("rotated root onboards after finalize");
}

#[test]
fn finalized_program_without_a_root_fails_closed() {
    let mut env = Env::bare();
    set_upgrade_authority(&mut env.svm, None);
    let auth = env.authority.insecure_clone();
    let other = env.funded();
    for signer in [&auth, &other] {
        assert_eq!(
            env.seed_root_as(signer, signer.pubkey()),
            Err(gate(GateError::NotUpgradeAuthority))
        );
    }
    assert!(env.root().is_none());
}

#[test]
fn root_can_be_seeded_straight_to_a_vault() {
    let mut env = Env::bare();
    let auth = env.authority.insecure_clone();
    let vault = env.funded();
    env.seed_root_as(&auth, vault.pubkey())
        .expect("upgrade key seeds the root to a key it does not control");
    let mint = Pubkey::new_unique();
    assert_eq!(
        env.init_as(&auth, &mint),
        Err(gate(GateError::NotRootAuthority)),
        "the upgrade key has no onboarding power once the root names a vault"
    );
    env.init_as(&vault, &mint).expect("vault onboards");
}

#[test]
fn root_hand_over_is_two_step() {
    let mut env = Env::new();
    let old = env.authority.insecure_clone();
    let (next, attacker) = (env.funded(), env.funded());

    assert_eq!(
        env.accept_root(&next),
        Err(gate(GateError::NoPendingRootAuthority))
    );
    assert_eq!(
        env.cancel_root(&old),
        Err(gate(GateError::NoPendingRootAuthority))
    );
    assert_eq!(
        env.propose_root(&attacker, attacker.pubkey()),
        Err(gate(GateError::NotRootAuthority))
    );
    for bad in [Pubkey::default(), old.pubkey()] {
        assert_eq!(
            env.propose_root(&old, bad),
            Err(gate(GateError::InvalidNewAuthority))
        );
    }

    env.propose_root(&old, next.pubkey()).expect("propose");
    assert_eq!(env.root().unwrap().authority, old.pubkey(), "not yet");
    assert_eq!(
        env.accept_root(&attacker),
        Err(gate(GateError::NotProposedAuthority))
    );
    env.cancel_root(&old).expect("cancel");
    assert_eq!(
        env.accept_root(&next),
        Err(gate(GateError::NoPendingRootAuthority)),
        "a cancelled proposal cannot be accepted"
    );

    env.propose_root(&old, next.pubkey()).expect("re-propose");
    env.accept_root(&next).expect("accept");
    let root = env.root().unwrap();
    assert_eq!(root.authority, next.pubkey());
    assert_eq!(root.pending_authority, Pubkey::default());

    let mint = Pubkey::new_unique();
    assert_eq!(
        env.init_as(&old, &mint),
        Err(gate(GateError::NotRootAuthority)),
        "the old root no longer onboards"
    );
    env.init_as(&next, &mint).expect("the new root onboards");
}

#[test]
fn config_initializes_once_per_mint() {
    let mut env = Env::new();
    let auth = env.authority.insecure_clone();
    let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
    env.init_as(&auth, &a).expect("first mint");
    env.init_as(&auth, &b)
        .expect("second mint gets its own config");
    // System program `AccountAlreadyInUse` (code 0) from anchor's `init`.
    assert_eq!(
        env.init_as(&auth, &a),
        Err(Fail::Code(0)),
        "re-init must fail"
    );
}
