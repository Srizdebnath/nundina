//! #53: config values outside the hard bounds are rejected at init **and**
//! at update, so no config can be put in a state where a later `validate`
//! check passes vacuously. Attester edits fail loudly instead of no-op.
//! #95: zero seasoning is vacuous too, and the holder cap may not reach the
//! §12(g) trigger. #96: an update that writes nothing is rejected.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use common::{deploy, root_pda, seed_root, PROGRAM_ID};
use credit_gate::{
    accounts as acc, instruction as ix, GateError, MarketConfig, MAX_DURATION_SECONDS,
    MAX_HOLDER_CAP, MIN_SEASONING_SECONDS, MIN_SETTLEMENT_HORIZON_SECONDS,
};
use litesvm::LiteSVM;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

fn config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[MarketConfig::SEED, mint.as_ref()], &PROGRAM_ID).0
}

#[derive(Clone)]
struct Init {
    holder_cap: u32,
    seasoning: i64,
    horizon: i64,
    attesters: Vec<Pubkey>,
}

impl Default for Init {
    fn default() -> Self {
        Init {
            holder_cap: 8,
            seasoning: 7 * 86_400,
            horizon: 86_400,
            attesters: vec![],
        }
    }
}

#[derive(Default, Clone)]
struct Update {
    holder_cap: Option<u32>,
    seasoning: Option<i64>,
    horizon: Option<i64>,
    add: Option<Pubkey>,
    remove: Option<Pubkey>,
    blocklist: Option<Pubkey>,
}

struct Env {
    svm: LiteSVM,
    auth: Keypair,
}

impl Env {
    fn new() -> Env {
        let auth = Keypair::new();
        let mut svm = deploy(&auth);
        seed_root(&mut svm, &auth);
        Env { svm, auth }
    }

    fn send(&mut self, ix: Instruction) -> Result<(), u32> {
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&self.auth.pubkey()),
            &[&self.auth],
            self.svm.latest_blockhash(),
        );
        let out = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        out.map(|_| ()).map_err(|f| match f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(c)) => c,
            other => panic!("unexpected failure {other:?} | {:?}", f.meta.logs),
        })
    }

    fn init(&mut self, mint: &Pubkey, p: Init) -> Result<(), u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeConfig {
                config: config_pda(mint),
                root: root_pda(),
                mint: *mint,
                authority: self.auth.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::InitializeConfig {
                policy_class: 1,
                holder_cap: p.holder_cap,
                seasoning_seconds: p.seasoning,
                settlement_horizon_seconds: p.horizon,
                attesters: p.attesters,
                identity_schema: Pubkey::new_unique(),
                accreditation_schema: Pubkey::new_unique(),
            }
            .data(),
        };
        self.send(ix)
    }

    fn update(&mut self, mint: &Pubkey, u: Update) -> Result<(), u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::UpdateConfig {
                config: config_pda(mint),
                mint: *mint,
                gate_config_authority: self.auth.pubkey(),
            }
            .to_account_metas(None),
            data: ix::UpdateConfig {
                policy_class: None,
                holder_cap: u.holder_cap,
                seasoning_seconds: u.seasoning,
                settlement_horizon_seconds: u.horizon,
                attester_add: u.add,
                attester_remove: u.remove,
                blocklist_set: u.blocklist,
            }
            .data(),
        };
        self.send(ix)
    }

    fn config(&self, mint: &Pubkey) -> Option<MarketConfig> {
        let a = self.svm.get_account(&config_pda(mint))?;
        (!a.data.is_empty()).then(|| MarketConfig::try_deserialize(&mut a.data.as_slice()).unwrap())
    }
}

fn gate(e: GateError) -> u32 {
    u32::from(e)
}

#[test]
fn init_rejects_out_of_bounds_values() {
    let mut env = Env::new();
    let oob = Err(gate(GateError::ConfigOutOfBounds));
    let cases = [
        Init {
            holder_cap: 0,
            ..Init::default()
        },
        Init {
            holder_cap: MAX_HOLDER_CAP + 1,
            ..Init::default()
        },
        // #95: the §12(g) trigger itself is out of bounds (doc 07 L7).
        Init {
            holder_cap: 2_000,
            ..Init::default()
        },
        Init {
            seasoning: -1,
            ..Init::default()
        },
        // #95: zero seasoning passes `acquired + 0 ≤ now` for every lot.
        Init {
            seasoning: 0,
            ..Init::default()
        },
        Init {
            seasoning: MIN_SEASONING_SECONDS - 1,
            ..Init::default()
        },
        Init {
            seasoning: MAX_DURATION_SECONDS + 1,
            ..Init::default()
        },
        Init {
            horizon: 0,
            ..Init::default()
        },
        Init {
            horizon: 1,
            ..Init::default()
        },
        Init {
            horizon: MIN_SETTLEMENT_HORIZON_SECONDS - 1,
            ..Init::default()
        },
        Init {
            horizon: -86_400,
            ..Init::default()
        },
        Init {
            horizon: MAX_DURATION_SECONDS + 1,
            ..Init::default()
        },
    ];
    for c in cases {
        let mint = Pubkey::new_unique();
        assert_eq!(env.init(&mint, c), oob);
        assert!(env.config(&mint).is_none(), "rejected init leaves no PDA");
    }
    let dup = Pubkey::new_unique();
    let mint = Pubkey::new_unique();
    assert_eq!(
        env.init(
            &mint,
            Init {
                attesters: vec![dup, dup],
                ..Init::default()
            }
        ),
        Err(gate(GateError::DuplicateAttester))
    );
    // Boundary values are accepted, at both ends of each range.
    assert_eq!(MAX_HOLDER_CAP, 1_999, "below the 2 000 trigger");
    let mint = Pubkey::new_unique();
    env.init(
        &mint,
        Init {
            holder_cap: MAX_HOLDER_CAP,
            seasoning: MIN_SEASONING_SECONDS,
            horizon: MAX_DURATION_SECONDS,
            attesters: vec![],
        },
    )
    .expect("boundary values are in bounds");
    let mint = Pubkey::new_unique();
    env.init(
        &mint,
        Init {
            holder_cap: 1,
            seasoning: MAX_DURATION_SECONDS,
            horizon: MIN_SETTLEMENT_HORIZON_SECONDS,
            attesters: vec![],
        },
    )
    .expect("boundary values are in bounds");
}

#[test]
fn update_rejects_out_of_bounds_values_and_keeps_state() {
    let mut env = Env::new();
    let mint = Pubkey::new_unique();
    env.init(&mint, Init::default()).unwrap();
    let before = env.config(&mint).unwrap();
    let oob = Err(gate(GateError::ConfigOutOfBounds));
    for u in [
        Update {
            holder_cap: Some(0),
            ..Update::default()
        },
        Update {
            holder_cap: Some(u32::MAX),
            ..Update::default()
        },
        Update {
            holder_cap: Some(2_000),
            ..Update::default()
        },
        Update {
            seasoning: Some(-1),
            ..Update::default()
        },
        Update {
            seasoning: Some(0),
            ..Update::default()
        },
        Update {
            horizon: Some(-1),
            ..Update::default()
        },
        Update {
            horizon: Some(0),
            ..Update::default()
        },
        Update {
            horizon: Some(1),
            ..Update::default()
        },
        Update {
            seasoning: Some(i64::MAX),
            ..Update::default()
        },
    ] {
        assert_eq!(env.update(&mint, u), oob);
    }
    let after = env.config(&mint).unwrap();
    assert_eq!(after.holder_cap, before.holder_cap);
    assert_eq!(after.seasoning_seconds, before.seasoning_seconds);
    assert_eq!(
        after.settlement_horizon_seconds,
        before.settlement_horizon_seconds
    );
}

#[test]
fn update_that_writes_nothing_is_rejected() {
    let mut env = Env::new();
    let mint = Pubkey::new_unique();
    env.init(&mint, Init::default()).unwrap();
    assert_eq!(
        env.update(&mint, Update::default()),
        Err(gate(GateError::EmptyUpdate))
    );
}

#[test]
fn blocklist_rejects_degenerate_targets() {
    let mut env = Env::new();
    let mint = Pubkey::new_unique();
    env.init(&mint, Init::default()).unwrap();
    for bad in [Pubkey::default(), mint, config_pda(&mint)] {
        assert_eq!(
            env.update(
                &mint,
                Update {
                    blocklist: Some(bad),
                    ..Update::default()
                }
            ),
            Err(gate(GateError::InvalidBlocklist))
        );
    }
    assert!(env.config(&mint).unwrap().blocklist.is_none());
    let bl = Pubkey::new_unique();
    env.update(
        &mint,
        Update {
            blocklist: Some(bl),
            ..Update::default()
        },
    )
    .expect("a real block-list key is accepted");
    assert_eq!(env.config(&mint).unwrap().blocklist, Some(bl));
}

#[test]
fn attester_edits_fail_loudly() {
    let mut env = Env::new();
    let mint = Pubkey::new_unique();
    let set: Vec<Pubkey> = (0..4).map(|_| Pubkey::new_unique()).collect();
    env.init(
        &mint,
        Init {
            attesters: set.clone(),
            ..Init::default()
        },
    )
    .unwrap();

    assert_eq!(
        env.update(
            &mint,
            Update {
                add: Some(Pubkey::new_unique()),
                ..Update::default()
            }
        ),
        Err(gate(GateError::TooManyAttesters)),
        "full set"
    );
    assert_eq!(
        env.update(
            &mint,
            Update {
                remove: Some(Pubkey::new_unique()),
                ..Update::default()
            }
        ),
        Err(gate(GateError::AttesterNotFound))
    );
    // Swap in one call on a full set: remove runs before add.
    let fresh = Pubkey::new_unique();
    env.update(
        &mint,
        Update {
            remove: Some(set[1]),
            add: Some(fresh),
            ..Update::default()
        },
    )
    .expect("swap");
    let cfg = env.config(&mint).unwrap();
    assert_eq!(cfg.attester_count, 4);
    assert!(cfg.attesters.contains(&fresh) && !cfg.attesters.contains(&set[1]));

    assert_eq!(
        env.update(
            &mint,
            Update {
                remove: Some(set[0]),
                add: Some(set[2]),
                ..Update::default()
            }
        ),
        Err(gate(GateError::DuplicateAttester))
    );
    assert_eq!(env.config(&mint).unwrap().attester_count, 4, "rolled back");
}
