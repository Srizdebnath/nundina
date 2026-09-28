//! #49 regression: `ConfigUpdated.new_config_hash` covers every governed
//! field. A block-list-only update must emit a different hash, and the hash
//! must equal `MarketConfig::fingerprint()` of the stored account.

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, AnchorDeserialize, Discriminator};
use anchor_lang::{InstructionData, ToAccountMetas};
use base64::Engine;
use credit_gate::{accounts as acc, instruction as ix, ConfigUpdated, MarketConfig};
use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

const PROGRAM_ID: Pubkey = credit_gate::ID;
const LOADER_V3: Pubkey = anchor_lang::solana_program::bpf_loader_upgradeable::ID;

fn setup() -> (LiteSVM, Keypair, Pubkey) {
    let so = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../target/deploy/credit_gate.so"
    );
    let bytes = std::fs::read(so)
        .unwrap_or_else(|e| panic!("{so}: {e} — run `anchor build` at the repo root first"));
    let mut svm = LiteSVM::new();
    svm.add_program(PROGRAM_ID, &bytes).expect("load program");
    let authority = Keypair::new();
    let (programdata, _) = Pubkey::find_program_address(&[PROGRAM_ID.as_ref()], &LOADER_V3);
    let mut pd = svm.get_account(&programdata).expect("programdata");
    pd.data[12] = 1;
    pd.data[13..45].copy_from_slice(authority.pubkey().as_ref());
    svm.set_account(programdata, pd).unwrap();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();
    (svm, authority, programdata)
}

fn config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[MarketConfig::SEED, mint.as_ref()], &PROGRAM_ID).0
}

/// Sends `ix` and returns the single `ConfigUpdated` it emitted.
fn send(svm: &mut LiteSVM, ix: Instruction, signer: &Keypair) -> ConfigUpdated {
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&signer.pubkey()),
        &[signer],
        svm.latest_blockhash(),
    );
    let meta = svm.send_transaction(tx).expect("tx succeeds");
    svm.expire_blockhash();
    let events: Vec<ConfigUpdated> = meta
        .logs
        .iter()
        .filter_map(|l| l.strip_prefix("Program data: "))
        .filter_map(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .filter(|d| d.starts_with(ConfigUpdated::DISCRIMINATOR))
        .map(|d| ConfigUpdated::try_from_slice(&d[ConfigUpdated::DISCRIMINATOR.len()..]).unwrap())
        .collect();
    assert_eq!(events.len(), 1, "exactly one ConfigUpdated (CG-P7)");
    events.into_iter().next().unwrap()
}

fn stored(svm: &LiteSVM, mint: &Pubkey) -> MarketConfig {
    let a = svm.get_account(&config_pda(mint)).expect("config exists");
    MarketConfig::try_deserialize(&mut a.data.as_slice()).unwrap()
}

fn update_blocklist(mint: &Pubkey, auth: &Pubkey, bl: Pubkey) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::UpdateConfig {
            config: config_pda(mint),
            mint: *mint,
            gate_config_authority: *auth,
        }
        .to_account_metas(None),
        data: ix::UpdateConfig {
            policy_class: None,
            holder_cap: None,
            seasoning_seconds: None,
            settlement_horizon_seconds: None,
            attester_add: None,
            attester_remove: None,
            blocklist_set: Some(bl),
        }
        .data(),
    }
}

#[test]
fn blocklist_only_update_changes_the_provenance_hash() {
    let (mut svm, auth, programdata) = setup();
    let mint = Pubkey::new_unique();
    let init = Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::InitializeConfig {
            config: config_pda(&mint),
            mint,
            authority: auth.pubkey(),
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
            attesters: vec![Pubkey::new_unique()],
            identity_schema: Pubkey::new_unique(),
            accreditation_schema: Pubkey::new_unique(),
        }
        .data(),
    };
    let e0 = send(&mut svm, init, &auth);
    assert_eq!(e0.new_config_hash, stored(&svm, &mint).fingerprint());

    let (bl_a, bl_b) = (Pubkey::new_unique(), Pubkey::new_unique());
    let e1 = send(
        &mut svm,
        update_blocklist(&mint, &auth.pubkey(), bl_a),
        &auth,
    );
    assert_ne!(
        e1.new_config_hash, e0.new_config_hash,
        "block-list set is visible"
    );
    assert_eq!(e1.new_config_hash, stored(&svm, &mint).fingerprint());

    let e2 = send(
        &mut svm,
        update_blocklist(&mint, &auth.pubkey(), bl_b),
        &auth,
    );
    assert_ne!(
        e2.new_config_hash, e1.new_config_hash,
        "block-list re-point is visible"
    );
    assert_eq!(e2.new_config_hash, stored(&svm, &mint).fingerprint());

    // The hash is a full 32-byte digest, not a padded 64-bit one.
    assert!(e2.new_config_hash[8..].iter().any(|b| *b != 0));
}
