//! #49 regression: `ConfigUpdated.new_config_hash` covers every governed
//! field. A block-list-only update must emit a different hash, and the hash
//! must equal `MarketConfig::fingerprint()` of the stored account.
//! #96: the event names its market (`mint`) and a real `field_group` mask.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, AnchorDeserialize, Discriminator};
use anchor_lang::{InstructionData, ToAccountMetas};
use base64::Engine;
use common::{deploy, root_pda, seed_root, PROGRAM_ID};
use credit_gate::field_group as fg;
use credit_gate::{accounts as acc, instruction as ix, ConfigUpdated, MarketConfig};
use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

fn setup() -> (LiteSVM, Keypair) {
    let authority = Keypair::new();
    let mut svm = deploy(&authority);
    seed_root(&mut svm, &authority);
    (svm, authority)
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

fn init_ix(mint: &Pubkey, auth: &Pubkey) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::InitializeConfig {
            config: config_pda(mint),
            root: root_pda(),
            mint: *mint,
            authority: *auth,
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

fn update_ix(mint: &Pubkey, auth: &Pubkey, data: ix::UpdateConfig) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::UpdateConfig {
            config: config_pda(mint),
            mint: *mint,
            gate_config_authority: *auth,
        }
        .to_account_metas(None),
        data: data.data(),
    }
}

fn none() -> ix::UpdateConfig {
    ix::UpdateConfig {
        policy_class: None,
        holder_cap: None,
        seasoning_seconds: None,
        settlement_horizon_seconds: None,
        attester_add: None,
        attester_remove: None,
        blocklist_set: None,
    }
}

fn update_blocklist(mint: &Pubkey, auth: &Pubkey, bl: Pubkey) -> Instruction {
    update_ix(
        mint,
        auth,
        ix::UpdateConfig {
            blocklist_set: Some(bl),
            ..none()
        },
    )
}

#[test]
fn blocklist_only_update_changes_the_provenance_hash() {
    let (mut svm, auth) = setup();
    let mint = Pubkey::new_unique();
    let e0 = send(&mut svm, init_ix(&mint, &auth.pubkey()), &auth);
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

#[test]
fn config_updated_names_its_market_and_the_fields_written() {
    let (mut svm, auth) = setup();
    let (mint_a, mint_b) = (Pubkey::new_unique(), Pubkey::new_unique());

    let ea = send(&mut svm, init_ix(&mint_a, &auth.pubkey()), &auth);
    let eb = send(&mut svm, init_ix(&mint_b, &auth.pubkey()), &auth);
    assert_eq!(ea.mint, mint_a);
    assert_eq!(eb.mint, mint_b, "same authority, different markets");
    let init_mask = fg::INIT
        | fg::POLICY_CLASS
        | fg::HOLDER_CAP
        | fg::SEASONING
        | fg::SETTLEMENT_HORIZON
        | fg::ATTESTERS
        | fg::SCHEMAS;
    assert_eq!(ea.field_group, init_mask);

    // One bit per supplied group; never INIT on an update.
    let cases: [(ix::UpdateConfig, u8); 4] = [
        (
            ix::UpdateConfig {
                holder_cap: Some(9),
                ..none()
            },
            fg::HOLDER_CAP,
        ),
        (
            ix::UpdateConfig {
                policy_class: Some(2),
                ..none()
            },
            fg::POLICY_CLASS,
        ),
        (
            ix::UpdateConfig {
                attester_add: Some(Pubkey::new_unique()),
                seasoning_seconds: Some(8 * 86_400),
                ..none()
            },
            fg::ATTESTERS | fg::SEASONING,
        ),
        (
            ix::UpdateConfig {
                settlement_horizon_seconds: Some(2 * 86_400),
                blocklist_set: Some(Pubkey::new_unique()),
                ..none()
            },
            fg::SETTLEMENT_HORIZON | fg::BLOCKLIST,
        ),
    ];
    for (data, want) in cases {
        let e = send(&mut svm, update_ix(&mint_b, &auth.pubkey(), data), &auth);
        assert_eq!(e.mint, mint_b);
        assert_eq!(e.field_group, want);
        assert_eq!(e.new_config_hash, stored(&svm, &mint_b).fingerprint());
    }
}
