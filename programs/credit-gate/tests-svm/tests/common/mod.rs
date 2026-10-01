//! Shared by every test file: the gate root (#93) must be seeded before any
//! `initialize_config`, so each harness seeds it the same way.
#![allow(dead_code)] // each test binary uses a different subset

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{InstructionData, ToAccountMetas};
use credit_gate::{accounts as acc, instruction as ix, GateRoot};
use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

pub const PROGRAM_ID: Pubkey = credit_gate::ID;
pub const LOADER_V3: Pubkey = anchor_lang::solana_program::bpf_loader_upgradeable::ID;

pub fn root_pda() -> Pubkey {
    Pubkey::find_program_address(&[GateRoot::SEED], &PROGRAM_ID).0
}

pub fn programdata_pda() -> Pubkey {
    Pubkey::find_program_address(&[PROGRAM_ID.as_ref()], &LOADER_V3).0
}

/// Fresh VM with the real `anchor build` artifact loaded, and `upgrade`
/// installed as its upgrade authority (LiteSVM deploys with none).
/// ProgramData header: u32 tag | u64 slot | Option<Pubkey>.
pub fn deploy(upgrade: &Keypair) -> LiteSVM {
    let so = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../target/deploy/credit_gate.so"
    );
    let bytes = std::fs::read(so)
        .unwrap_or_else(|e| panic!("{so}: {e} — run `anchor build` at the repo root first"));
    let mut svm = LiteSVM::new();
    svm.add_program(PROGRAM_ID, &bytes).expect("load program");
    set_upgrade_authority(&mut svm, Some(&upgrade.pubkey()));
    svm.airdrop(&upgrade.pubkey(), 10_000_000_000).unwrap();
    svm
}

/// Rewrites the ProgramData upgrade authority; `None` = finalized.
pub fn set_upgrade_authority(svm: &mut LiteSVM, authority: Option<&Pubkey>) {
    let pd_key = programdata_pda();
    let mut pd = svm.get_account(&pd_key).expect("programdata");
    match authority {
        Some(a) => {
            pd.data[12] = 1;
            pd.data[13..45].copy_from_slice(a.as_ref());
        }
        None => {
            pd.data[12] = 0;
            pd.data[13..45].fill(0);
        }
    }
    svm.set_account(pd_key, pd).unwrap();
}

pub fn init_root_ix(signer: &Pubkey, root_authority: Pubkey, program_data: Pubkey) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::InitializeRoot {
            root: root_pda(),
            upgrade_authority: *signer,
            program: PROGRAM_ID,
            program_data,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: ix::InitializeRoot {
            authority: root_authority,
        }
        .data(),
    }
}

/// Seeds the root with `upgrade` as both seeder and root authority — the
/// shape every pre-#93 test assumed (the upgrade key creates configs).
pub fn seed_root(svm: &mut LiteSVM, upgrade: &Keypair) {
    let ix = init_root_ix(&upgrade.pubkey(), upgrade.pubkey(), programdata_pda());
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&upgrade.pubkey()),
        &[upgrade],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("seed gate root");
    svm.expire_blockhash();
}
