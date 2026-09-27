//! #26 regression: `initialize_config` is gated on the program's upgrade
//! authority, so the first-caller-wins `init` of a mint's `MarketConfig`
//! cannot be front-run into a permanent gate-authority takeover.
//!
//! Loads the real `anchor build` artifact as an upgradeable program.

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use credit_gate::{accounts as acc, instruction as ix, GateError, MarketConfig};
use litesvm::LiteSVM;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

const PROGRAM_ID: Pubkey = credit_gate::ID;
const LOADER_V3: Pubkey = anchor_lang::solana_program::bpf_loader_upgradeable::ID;
/// anchor `ConstraintRaw` — a `constraint = …` without a custom error.
const CONSTRAINT_RAW: u32 = 2003;

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
    authority: Keypair,
    programdata: Pubkey,
}

impl Env {
    /// Fresh VM with the program deployed; `authority` is installed as its
    /// upgrade authority when `upgradeable`, otherwise the program is final.
    fn with(upgradeable: bool) -> Env {
        let so = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../target/deploy/credit_gate.so"
        );
        let bytes = std::fs::read(so)
            .unwrap_or_else(|e| panic!("{so}: {e} — run `anchor build` at the repo root first"));
        let mut svm = LiteSVM::new();
        svm.add_program(PROGRAM_ID, &bytes).expect("load program");

        // LiteSVM deploys with no upgrade authority; install one as a real
        // deploy would. ProgramData header: u32 tag | u64 slot | Option<Pubkey>.
        let authority = Keypair::new();
        let (programdata, _) = Pubkey::find_program_address(&[PROGRAM_ID.as_ref()], &LOADER_V3);
        if upgradeable {
            let mut pd = svm.get_account(&programdata).expect("programdata");
            pd.data[12] = 1;
            pd.data[13..45].copy_from_slice(authority.pubkey().as_ref());
            svm.set_account(programdata, pd).unwrap();
        }
        svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();
        Env {
            svm,
            authority,
            programdata,
        }
    }

    fn new() -> Env {
        Env::with(true)
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

    fn init_ix(&self, signer: &Pubkey, mint: &Pubkey, program_data: Pubkey) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeConfig {
                config: config_pda(mint),
                mint: *mint,
                authority: *signer,
                program: PROGRAM_ID,
                program_data,
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
        let ix = self.init_ix(&signer.pubkey(), mint, self.programdata);
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
}

fn config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[MarketConfig::SEED, mint.as_ref()], &PROGRAM_ID).0
}

#[test]
fn upgrade_authority_initializes_and_governs() {
    let mut env = Env::new();
    let mint = Pubkey::new_unique();
    let auth = env.authority.insecure_clone();
    env.init_as(&auth, &mint)
        .expect("upgrade authority initializes");

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
        Err(gate(GateError::NotUpgradeAuthority))
    );
    assert!(
        env.config(&mint).is_none(),
        "rejected init must not create the PDA"
    );

    // The legitimate call still lands after the failed front-run.
    let auth = env.authority.insecure_clone();
    env.init_as(&auth, &mint)
        .expect("upgrade authority initializes");
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
fn spoofed_program_data_is_rejected() {
    let mut env = Env::new();
    let mint = Pubkey::new_unique();
    let attacker = env.funded();

    // A loader-owned ProgramData-shaped account naming the attacker as
    // upgrade authority, at an address that is not this program's.
    let mut fake = env.svm.get_account(&env.programdata).unwrap();
    fake.data[12] = 1;
    fake.data[13..45].copy_from_slice(attacker.pubkey().as_ref());
    let fake_key = Pubkey::new_unique();
    env.svm.set_account(fake_key, fake).unwrap();

    let ix = env.init_ix(&attacker.pubkey(), &mint, fake_key);
    assert_eq!(
        env.send(&[ix], &[&attacker]),
        Err(Fail::Code(CONSTRAINT_RAW))
    );
    assert!(env.config(&mint).is_none());
}

#[test]
fn immutable_program_admits_no_initializer() {
    let mut env = Env::with(false);
    let mint = Pubkey::new_unique();
    let auth = env.authority.insecure_clone();
    let other = env.funded();
    for signer in [&auth, &other] {
        assert_eq!(
            env.init_as(signer, &mint),
            Err(gate(GateError::NotUpgradeAuthority))
        );
    }
    assert!(env.config(&mint).is_none());
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
