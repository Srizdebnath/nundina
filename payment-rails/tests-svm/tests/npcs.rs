//! R5 token leg: the rails against a mint built to the NPCS extension
//! manifest (token/params.toml, doc 09 §2) — DefaultAccountState=Frozen,
//! PermanentDelegate, MetadataPointer(self) + TokenMetadata — using the real
//! Token-2022 instructions, not fabricated state. A `gate` keypair holds the
//! freeze authority and stands in for credit-gate's thaw (PHASES §4.3).

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_spl::token_2022::spl_token_2022::{
    self,
    extension::{
        default_account_state, metadata_pointer, BaseStateWithExtensions, ExtensionType,
        StateWithExtensions,
    },
    state::{Account as TokenAccount, AccountState, Mint},
};
use anchor_spl::token_2022_extensions::spl_token_metadata_interface;
use common::*;
use solana_account::Account;
use solana_keypair::Keypair;
use solana_signer::Signer;

const T22: Pubkey = anchor_spl::token_2022::ID;
const UNIT: u64 = 1_000_000;
const PRICE: u64 = 98_123_456;
/// Token-2022 `TokenError::AccountFrozen`.
const ACCOUNT_FROZEN: u32 = 17;

/// Env whose asset is an NPCS-manifest mint; `gate` is its freeze
/// authority, permanent delegate and mint authority.
struct Npcs {
    env: Env,
    gate: Keypair,
}

impl Npcs {
    fn new(tp: Tp) -> Npcs {
        let mut env = Env::new(tp);
        let gate = env.funded();
        let mint = Pubkey::new_unique();
        let exts = [
            ExtensionType::DefaultAccountState,
            ExtensionType::PermanentDelegate,
            ExtensionType::MetadataPointer,
        ];
        let len = ExtensionType::try_calculate_account_len::<Mint>(&exts).unwrap();
        // Pre-fund for the metadata TLV the Initialize below appends.
        blank(&mut env, mint, len, len + 1024);
        let g = gate.pubkey();
        let ixs = vec![
            default_account_state::instruction::initialize_default_account_state(
                &T22,
                &mint,
                &AccountState::Frozen,
            )
            .unwrap(),
            spl_token_2022::instruction::initialize_permanent_delegate(&T22, &mint, &g).unwrap(),
            metadata_pointer::instruction::initialize(&T22, &mint, Some(g), Some(mint)).unwrap(),
            spl_token_2022::instruction::initialize_mint2(&T22, &mint, &g, Some(&g), 6).unwrap(),
            spl_token_metadata_interface::instruction::initialize(
                &T22,
                &mint,
                &g,
                &mint,
                &g,
                "NUNDINA Pilot Credit Sleeve".into(),
                "NPCS".into(),
                "https://nundina.com/npcs/metadata.json".into(),
            ),
        ];
        env.send(&ixs, &[&gate]).expect("NPCS mint");
        env.asset_mint = mint;
        Npcs { env, gate }
    }

    /// A holder account created by the real InitializeAccount3 — so it
    /// starts Frozen — then thawed by the gate and minted `amount`.
    fn holder_account(&mut self, owner: &Pubkey, amount: u64) -> Pubkey {
        let a = self.new_account(owner);
        self.thaw(&a);
        if amount > 0 {
            let ix = spl_token_2022::instruction::mint_to_checked(
                &T22,
                &self.env.asset_mint,
                &a,
                &self.gate.pubkey(),
                &[],
                amount,
                6,
            )
            .unwrap();
            let g = self.gate.insecure_clone();
            self.env.send(&[ix], &[&g]).expect("mint NPCS");
        }
        a
    }

    fn new_account(&mut self, owner: &Pubkey) -> Pubkey {
        let key = Pubkey::new_unique();
        let len = ExtensionType::try_calculate_account_len::<TokenAccount>(&[]).unwrap();
        blank(&mut self.env, key, len, len);
        let ix = spl_token_2022::instruction::initialize_account3(
            &T22,
            &key,
            &self.env.asset_mint,
            owner,
        )
        .unwrap();
        let g = self.gate.insecure_clone();
        self.env.send(&[ix], &[&g]).expect("init NPCS account");
        key
    }

    fn thaw(&mut self, account: &Pubkey) {
        let ix = spl_token_2022::instruction::thaw_account(
            &T22,
            account,
            &self.env.asset_mint,
            &self.gate.pubkey(),
            &[],
        )
        .unwrap();
        let g = self.gate.insecure_clone();
        self.env.send(&[ix], &[&g]).expect("gate thaw");
    }

    fn state(&self, account: &Pubkey) -> AccountState {
        let a = self.env.svm.get_account(account).unwrap();
        StateWithExtensions::<TokenAccount>::unpack(&a.data)
            .unwrap()
            .base
            .state
    }

    /// Participant with USDC plus a gate-thawed NPCS account.
    fn party(&mut self, usdc: u64, npcs: u64) -> Party {
        let (key, usdc_acct) = self.env.bidder(usdc);
        let asset = self.holder_account(&key.pubkey(), npcs);
        Party {
            key,
            usdc: usdc_acct,
            asset,
        }
    }
}

/// An empty, rent-funded account owned by Token-2022, ready for Initialize*.
fn blank(env: &mut Env, key: Pubkey, len: usize, rent_for: usize) {
    let lamports = env.svm.minimum_balance_for_rent_exemption(rent_for);
    env.svm
        .set_account(
            key,
            Account {
                lamports,
                data: vec![0; len],
                owner: T22,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
}

#[test]
fn manifest_mint_is_real_npcs_shape() {
    let mut n = Npcs::new(Tp::Spl);
    let mint = n.env.svm.get_account(&n.env.asset_mint).unwrap();
    let m = StateWithExtensions::<Mint>::unpack(&mint.data).unwrap();
    let mut got = m.get_extension_types().unwrap();
    got.sort_by_key(|e| *e as u16);
    let mut want = vec![
        ExtensionType::DefaultAccountState,
        ExtensionType::PermanentDelegate,
        ExtensionType::MetadataPointer,
        ExtensionType::TokenMetadata,
    ];
    want.sort_by_key(|e| *e as u16);
    assert_eq!(got, want);

    // Every new account starts frozen until the gate thaws it.
    let owner = Pubkey::new_unique();
    let a = n.new_account(&owner);
    assert_eq!(n.state(&a), AccountState::Frozen);
    n.thaw(&a);
    assert_eq!(n.state(&a), AccountState::Initialized);
}

/// PHASES §4.3: a vault of a DefaultAccountState=Frozen mint is frozen from
/// birth. `deposit_ask` into it is refused with a typed reason (not a raw
/// token error) and leaves nothing behind but the empty vault.
#[test]
fn deposit_ask_into_unthawed_vault_is_refused_typed() {
    for tp in Tp::ALL {
        let mut n = Npcs::new(tp);
        let s = n.env.open_session(1);
        let seller = n.party(BOND, UNIT);
        n.env.open_asset_vault(&s, &seller).unwrap();
        let vault = asset_vault_pda(&escrow_pda(&s, &seller.key.pubkey()));
        assert_eq!(n.state(&vault), AccountState::Frozen);
        assert_eq!(
            n.env.deposit_ask_only(&s, &seller, UNIT),
            Err(Fail::rails(payment_rails::RailsError::AssetVaultFrozen))
        );
        // One-shot open + deposit (the unfrozen-mint path) fails the same way.
        let other = n.party(BOND, UNIT);
        assert_eq!(
            n.env.deposit_ask(&s, &other, UNIT),
            Err(Fail::rails(payment_rails::RailsError::AssetVaultFrozen))
        );
        assert_eq!(n.env.bal(&seller.asset), UNIT);
        assert_eq!(n.env.session(&s).open_escrows, 0);
    }
}

/// The R5 NPCS path end to end: open vault → gate thaws → deposit_ask →
/// DvP fill into a gate-admitted buyer → finish → refunds. Every asset
/// unit and micro-USDC is conserved; the escrow vaults end closed.
#[test]
fn npcs_dvp_end_to_end_after_gate_thaw() {
    for tp in Tp::ALL {
        let mut n = Npcs::new(tp);
        n.env.set_fees(10, 5).unwrap();
        let s = n.env.open_session(1);
        let seller = n.party(BOND, 2 * UNIT);
        let buyer = n.party(300_000_000 + BOND, 0);

        n.env.open_asset_vault(&s, &seller).unwrap();
        let se = escrow_pda(&s, &seller.key.pubkey());
        n.thaw(&asset_vault_pda(&se));
        n.env.deposit_ask_only(&s, &seller, 2 * UNIT).unwrap();
        n.env
            .deposit(&s, &buyer.key, &buyer.usdc, 300_000_000)
            .unwrap();

        let be = escrow_pda(&s, &buyer.key.pubkey());
        let usdc = [
            seller.usdc,
            buyer.usdc,
            n.env.fee,
            n.env.reserve,
            vault_pda(&se),
            vault_pda(&be),
        ];
        let asset = [seller.asset, buyer.asset, asset_vault_pda(&se)];
        let (usdc0, asset0) = (n.env.total(&usdc), n.env.total(&asset));

        n.env.set_time(COMMIT_END);
        n.env.mark_revealed(&s, &seller.key.pubkey()).unwrap();
        n.env.mark_revealed(&s, &buyer.key.pubkey()).unwrap();
        n.env.set_time(REVEAL_END);
        let f = n.env.fill(&s, &buyer, &seller);
        n.env.settle(&f, 0, UNIT, PRICE).unwrap();
        assert_eq!(n.env.bal(&buyer.asset), UNIT);
        assert_eq!((n.env.total(&usdc), n.env.total(&asset)), (usdc0, asset0));

        n.env.finish_session(&s).unwrap();
        let crank = n.env.funded();
        n.env.refund_ask(&crank, &s, &seller).unwrap();
        n.env
            .refund(&crank, &s, &buyer.key.pubkey(), &buyer.usdc)
            .unwrap();
        assert_eq!((n.env.total(&usdc), n.env.total(&asset)), (usdc0, asset0));
        assert_eq!(n.env.bal(&seller.asset), UNIT, "unsold unit back to seller");
        assert!(!n.env.exists(&asset_vault_pda(&se)), "asset vault closed");
        assert_eq!(n.env.session(&s).open_escrows, 0);
    }
}

/// Delivery is gated by the gate, not the rails: a buyer whose NPCS account
/// was never thawed (not admitted) can't receive, and the fill doesn't land.
#[test]
fn fill_into_unadmitted_buyer_does_not_land() {
    let mut n = Npcs::new(Tp::Spl);
    let s = n.env.open_session(1);
    let seller = n.party(BOND, UNIT);
    let (key, usdc) = n.env.bidder(200_000_000 + BOND);
    let frozen = n.new_account(&key.pubkey());
    let buyer = Party {
        key,
        usdc,
        asset: frozen,
    };
    n.env.open_asset_vault(&s, &seller).unwrap();
    n.thaw(&asset_vault_pda(&escrow_pda(&s, &seller.key.pubkey())));
    n.env.deposit_ask_only(&s, &seller, UNIT).unwrap();
    n.env
        .deposit(&s, &buyer.key, &buyer.usdc, 200_000_000)
        .unwrap();
    n.env.set_time(COMMIT_END);
    n.env.mark_revealed(&s, &seller.key.pubkey()).unwrap();
    n.env.mark_revealed(&s, &buyer.key.pubkey()).unwrap();
    n.env.set_time(REVEAL_END);
    let f = n.env.fill(&s, &buyer, &seller);
    assert_eq!(
        n.env.settle(&f, 0, UNIT, PRICE),
        Err(Fail::Code(ACCOUNT_FROZEN))
    );
    assert!(
        !n.env.exists(&receipt_pda(&s, 0)),
        "no receipt: fill can be retried"
    );
    assert_eq!(n.env.session(&s).fills_settled, 0);
}

/// Opening the vault first must not open a griefing hole: donated asset
/// dust in the vault neither blocks `deposit_ask` nor inflates the escrow,
/// and refund_ask returns it to the seller.
#[test]
fn donated_dust_neither_blocks_nor_inflates_the_ask() {
    let mut n = Npcs::new(Tp::T22);
    let s = n.env.open_session(1);
    let seller = n.party(BOND, UNIT);
    let griefer = n.party(0, 7);
    n.env.open_asset_vault(&s, &seller).unwrap();
    let se = escrow_pda(&s, &seller.key.pubkey());
    let vault = asset_vault_pda(&se);
    n.thaw(&vault);
    n.env
        .raw_asset_transfer(&griefer.asset, &vault, &griefer.key, 7)
        .unwrap();
    n.env.deposit_ask_only(&s, &seller, UNIT).unwrap();
    assert_eq!(n.env.escrow(&s, &seller.key.pubkey()).asset, UNIT);
    assert_eq!(n.env.session(&s).total_asset, UNIT);
    n.env
        .fail_session_as(&n.env.authority.insecure_clone(), &s)
        .unwrap();
    let crank = n.env.funded();
    n.env.refund_ask(&crank, &s, &seller).unwrap();
    assert_eq!(n.env.bal(&seller.asset), UNIT + 7);
}

/// Only the owner can open their vault, and only while commits are open.
#[test]
fn open_asset_vault_is_owner_only_and_commit_window_only() {
    let mut n = Npcs::new(Tp::Spl);
    let s = n.env.open_session(1);
    let seller = n.party(BOND, UNIT);
    // A stranger signing as themselves can't aim at the seller's vault:
    // the escrow / vault seeds bind to the signer.
    let stranger = n.env.funded();
    let mut ix = n.env.open_asset_vault_ix(&s, &stranger.pubkey());
    let theirs = escrow_pda(&s, &seller.key.pubkey());
    ix.accounts[2].pubkey = theirs;
    ix.accounts[3].pubkey = asset_vault_pda(&theirs);
    assert!(n.env.send(&[ix], &[&stranger]).is_err());
    assert!(!n.env.exists(&asset_vault_pda(&theirs)));
    n.env.set_time(COMMIT_END);
    assert_eq!(
        n.env.open_asset_vault(&s, &seller),
        Err(Fail::rails(payment_rails::RailsError::OutsideWindow))
    );
}
