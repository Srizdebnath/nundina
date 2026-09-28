//! #50: `gate_config_authority` is rotatable by a two-step hand-over —
//! the current authority proposes, the proposed key accepts. Nobody else
//! (including the program upgrade authority, once it no longer governs the
//! config) can move it.

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use credit_gate::PendingGateAuthority;
use credit_gate::{accounts as acc, instruction as ix, GateError, MarketConfig};
use litesvm::LiteSVM;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

const PROGRAM_ID: Pubkey = credit_gate::ID;
const LOADER_V3: Pubkey = anchor_lang::solana_program::bpf_loader_upgradeable::ID;
/// anchor `ConstraintHasOne`.
const CONSTRAINT_HAS_ONE: u32 = 2001;
/// anchor `AccountNotInitialized`.
const ACCOUNT_NOT_INITIALIZED: u32 = 3012;

struct Env {
    svm: LiteSVM,
    upgrade: Keypair,
    mint: Pubkey,
}

fn config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[MarketConfig::SEED, mint.as_ref()], &PROGRAM_ID).0
}

fn pending_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PendingGateAuthority::SEED, mint.as_ref()], &PROGRAM_ID).0
}

impl Env {
    /// Program deployed, `upgrade` as upgrade authority, one config initialized.
    fn new() -> Env {
        let so = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../target/deploy/credit_gate.so"
        );
        let bytes = std::fs::read(so)
            .unwrap_or_else(|e| panic!("{so}: {e} — run `anchor build` at the repo root first"));
        let mut svm = LiteSVM::new();
        svm.add_program(PROGRAM_ID, &bytes).expect("load program");
        let upgrade = Keypair::new();
        let (programdata, _) = Pubkey::find_program_address(&[PROGRAM_ID.as_ref()], &LOADER_V3);
        let mut pd = svm.get_account(&programdata).expect("programdata");
        pd.data[12] = 1;
        pd.data[13..45].copy_from_slice(upgrade.pubkey().as_ref());
        svm.set_account(programdata, pd).unwrap();
        svm.airdrop(&upgrade.pubkey(), 10_000_000_000).unwrap();

        let mint = Pubkey::new_unique();
        let mut env = Env { svm, upgrade, mint };
        let init = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeConfig {
                config: config_pda(&mint),
                mint,
                authority: env.upgrade.pubkey(),
                program: PROGRAM_ID,
                program_data: programdata,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::InitializeConfig {
                policy_class: 1,
                holder_cap: 8,
                seasoning_seconds: 7 * 86_400,
                settlement_horizon_seconds: 86_400,
                attesters: vec![],
                identity_schema: Pubkey::new_unique(),
                accreditation_schema: Pubkey::new_unique(),
            }
            .data(),
        };
        let up = env.upgrade.insecure_clone();
        env.send(init, &up).expect("init");
        env
    }

    fn funded(&mut self) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        k
    }

    fn send(&mut self, ix: Instruction, signer: &Keypair) -> Result<(), u32> {
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&signer.pubkey()),
            &[signer],
            self.svm.latest_blockhash(),
        );
        let out = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        out.map(|_| ()).map_err(|f| match f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(c)) => c,
            other => panic!("unexpected failure {other:?} | {:?}", f.meta.logs),
        })
    }

    fn authority(&self) -> Pubkey {
        let a = self.svm.get_account(&config_pda(&self.mint)).unwrap();
        MarketConfig::try_deserialize(&mut a.data.as_slice())
            .unwrap()
            .gate_config_authority
    }

    fn pending_exists(&self) -> bool {
        self.svm
            .get_account(&pending_pda(&self.mint))
            .is_some_and(|a| !a.data.is_empty())
    }

    fn propose(&mut self, signer: &Keypair, new_authority: Pubkey) -> Result<(), u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::ProposeGateAuthority {
                config: config_pda(&self.mint),
                pending: pending_pda(&self.mint),
                mint: self.mint,
                gate_config_authority: signer.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::ProposeGateAuthority { new_authority }.data(),
        };
        self.send(ix, signer)
    }

    fn cancel(&mut self, signer: &Keypair, proposed_by: Pubkey) -> Result<(), u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::CancelGateAuthority {
                config: config_pda(&self.mint),
                pending: pending_pda(&self.mint),
                mint: self.mint,
                gate_config_authority: signer.pubkey(),
                proposed_by,
            }
            .to_account_metas(None),
            data: ix::CancelGateAuthority {}.data(),
        };
        self.send(ix, signer)
    }

    fn accept(&mut self, signer: &Keypair, proposed_by: Pubkey) -> Result<(), u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AcceptGateAuthority {
                config: config_pda(&self.mint),
                pending: pending_pda(&self.mint),
                mint: self.mint,
                new_authority: signer.pubkey(),
                proposed_by,
            }
            .to_account_metas(None),
            data: ix::AcceptGateAuthority {}.data(),
        };
        self.send(ix, signer)
    }

    fn set_holder_cap(&mut self, signer: &Keypair, cap: u32) -> Result<(), u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::UpdateConfig {
                config: config_pda(&self.mint),
                mint: self.mint,
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
        self.send(ix, signer)
    }
}

fn gate(e: GateError) -> u32 {
    u32::from(e)
}

#[test]
fn hand_over_moves_all_governance_to_the_new_key() {
    let mut env = Env::new();
    let old = env.upgrade.insecure_clone();
    let squads = env.funded();

    env.propose(&old, squads.pubkey()).expect("propose");
    assert!(env.pending_exists());
    // Proposing alone changes nothing.
    assert_eq!(env.authority(), old.pubkey());

    env.accept(&squads, old.pubkey()).expect("accept");
    assert_eq!(env.authority(), squads.pubkey());
    assert!(!env.pending_exists(), "pending closed on accept");

    // The new key governs; the old key (still the upgrade authority) does not.
    env.set_holder_cap(&squads, 9)
        .expect("new authority updates");
    assert_eq!(
        env.set_holder_cap(&old, 10),
        Err(gate(GateError::Unauthorized))
    );
    assert_eq!(
        env.propose(&old, old.pubkey()),
        Err(gate(GateError::Unauthorized))
    );
}

#[test]
fn only_the_current_authority_can_propose() {
    let mut env = Env::new();
    let attacker = env.funded();
    assert_eq!(
        env.propose(&attacker, attacker.pubkey()),
        Err(gate(GateError::Unauthorized))
    );
    assert!(!env.pending_exists());
}

#[test]
fn only_the_proposed_key_can_accept() {
    let mut env = Env::new();
    let old = env.upgrade.insecure_clone();
    let squads = env.funded();
    let attacker = env.funded();
    env.propose(&old, squads.pubkey()).unwrap();

    assert_eq!(
        env.accept(&attacker, old.pubkey()),
        Err(gate(GateError::NotProposedAuthority))
    );
    // Not even the proposer can accept on the new key's behalf.
    assert_eq!(
        env.accept(&old, old.pubkey()),
        Err(gate(GateError::NotProposedAuthority))
    );
    // Rent refund target is pinned to the proposer.
    assert_eq!(
        env.accept(&squads, attacker.pubkey()),
        Err(CONSTRAINT_HAS_ONE)
    );
    assert_eq!(env.authority(), old.pubkey());
}

#[test]
fn rejects_default_and_self_as_new_authority() {
    let mut env = Env::new();
    let old = env.upgrade.insecure_clone();
    assert_eq!(
        env.propose(&old, Pubkey::default()),
        Err(gate(GateError::InvalidNewAuthority))
    );
    assert_eq!(
        env.propose(&old, old.pubkey()),
        Err(gate(GateError::InvalidNewAuthority))
    );
}

#[test]
fn cancel_withdraws_and_allows_a_new_proposal() {
    let mut env = Env::new();
    let old = env.upgrade.insecure_clone();
    let (wrong, right) = (env.funded(), env.funded());
    let attacker = env.funded();

    env.propose(&old, wrong.pubkey()).unwrap();
    // One pending proposal per mint: `init` refuses a second (system code 0).
    assert_eq!(env.propose(&old, right.pubkey()), Err(0));
    assert_eq!(
        env.cancel(&attacker, old.pubkey()),
        Err(gate(GateError::Unauthorized))
    );
    env.cancel(&old, old.pubkey()).expect("cancel");
    assert!(!env.pending_exists());
    assert_eq!(
        env.accept(&wrong, old.pubkey()),
        Err(ACCOUNT_NOT_INITIALIZED),
        "a cancelled proposal cannot be accepted"
    );

    env.propose(&old, right.pubkey()).expect("re-propose");
    env.accept(&right, old.pubkey()).expect("accept");
    assert_eq!(env.authority(), right.pubkey());
}
