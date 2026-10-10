//! npcs-treasury (#148) against the real `npcs_treasury.so`: the treasury
//! signer PDA must be the asset's mint authority; a USDC deposit mints the
//! same value of NPCS; the issuer burns NPCS for vault USDC at the
//! published NAV (doc 09 Path 8, #202); every unit stays backed (I1) before
//! and after every mint and burn, so a mint from anywhere else stops the
//! treasury until it is repaired. With the gate accounts appended a deposit
//! also books the minted units as a `PrimaryMint` lot (I3.2, #202).

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AnchorDeserialize, Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::token_2022::spl_token_2022;
use base64::Engine;
use common::*;
use credit_gate::LotSource;
use npcs_treasury::{accounts as tacc, instruction as tix, Reconciled, Redeemed, TreasuryError};
use solana_account::Account;
use solana_keypair::Keypair;
use solana_program_pack::Pack;
use solana_signer::Signer;

const UNIT: u64 = 1_000_000;
const SYSTEM: Pubkey = anchor_lang::system_program::ID;

fn terr(e: TreasuryError) -> Fail {
    Fail::Code(u32::from(e))
}

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &npcs_treasury::ID).0
}

struct Fx {
    env: Env,
    asset: Pubkey,
    treasury: Pubkey,
    vault: Pubkey,
    signer: Pubkey,
    /// The asset feed's publisher (NAV 1.00 published at setup).
    publisher: Keypair,
}

/// A Token-2022 6-dp asset mint with supply 0 whose mint authority is
/// `authority` (the treasury signer PDA for the real tests).
fn asset_mint(env: &mut Env, authority: Pubkey) -> Pubkey {
    let key = Pubkey::new_unique();
    let mut data = vec![0u8; spl_token_2022::state::Mint::LEN];
    spl_token_2022::state::Mint {
        mint_authority: Some(authority).into(),
        supply: 0,
        decimals: 6,
        is_initialized: true,
        freeze_authority: None.into(),
    }
    .pack_into_slice(&mut data);
    let lamports = env.svm.minimum_balance_for_rent_exemption(data.len());
    env.svm
        .set_account(
            key,
            Account {
                lamports,
                data,
                owner: Tp::T22.id(),
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    key
}

fn init_ix(env: &Env, asset: &Pubkey) -> Instruction {
    let treasury = pda(&[npcs_treasury::TREASURY_SEED, asset.as_ref()]);
    Instruction {
        program_id: npcs_treasury::ID,
        accounts: tacc::Initialize {
            treasury,
            vault: pda(&[npcs_treasury::VAULT_SEED, treasury.as_ref()]),
            signer: npcs_treasury::signer_pda(&treasury).0,
            asset_mint: *asset,
            usdc_mint: env.mint,
            authority: env.authority.pubkey(),
            token_program: env.tp.id(),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: tix::Initialize {}.data(),
    }
}

fn fx(tp: Tp) -> Fx {
    let mut env = Env::new(tp);
    let so = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/deploy/npcs_treasury.so"
    );
    let bytes = std::fs::read(so).expect("run `anchor build` at the repo root");
    env.svm.add_program(npcs_treasury::ID, &bytes).unwrap();
    let a = env.authority.insecure_clone();
    // The asset must already have handed its mint authority over.
    let foreign = asset_mint(&mut env, a.pubkey());
    let ix = init_ix(&env, &foreign);
    assert_eq!(
        env.send(&[ix], &[&a]).unwrap_err(),
        terr(TreasuryError::NotMintAuthority)
    );
    // Derive the PDA for a fresh mint key, then create the mint with it.
    let key = Pubkey::new_unique();
    let treasury = pda(&[npcs_treasury::TREASURY_SEED, key.as_ref()]);
    let signer = npcs_treasury::signer_pda(&treasury).0;
    let tmp = asset_mint(&mut env, signer);
    let acct = env.svm.get_account(&tmp).unwrap();
    env.svm.set_account(key, acct).unwrap();
    let ix = init_ix(&env, &key);
    env.send(&[ix], &[&a]).expect("initialize");
    // The asset's mark-engine feed: the redemption NAV (doc 09 Path 8).
    env.load_mark_engine();
    let publisher = env.funded();
    env.init_feed(&key, feed_params(), &publisher.pubkey());
    env.publish_nav(&key, &publisher, NAV_ONE).expect("NAV");
    Fx {
        vault: pda(&[npcs_treasury::VAULT_SEED, treasury.as_ref()]),
        env,
        asset: key,
        treasury,
        signer,
        publisher,
    }
}

impl Fx {
    fn deposit(&mut self, who: &Keypair, usdc: &Pubkey, asset: &Pubkey, amount: u64) -> Res {
        let ix = self.deposit_ix(who, usdc, asset, amount);
        self.env.send(&[ix], &[who])
    }

    fn deposit_ix(&self, who: &Keypair, usdc: &Pubkey, asset: &Pubkey, amount: u64) -> Instruction {
        Instruction {
            program_id: npcs_treasury::ID,
            accounts: tacc::Deposit {
                treasury: self.treasury,
                vault: self.vault,
                signer: self.signer,
                asset_mint: self.asset,
                usdc_mint: self.env.mint,
                depositor: who.pubkey(),
                depositor_usdc: *usdc,
                depositor_asset: *asset,
                token_program: self.env.tp.id(),
                asset_token_program: Tp::T22.id(),
            }
            .to_account_metas(None),
            data: tix::Deposit { amount }.data(),
        }
    }

    fn redeem(&mut self, who: &Keypair, asset: &Pubkey, usdc: &Pubkey, amount: u64) -> Res {
        let ix = Instruction {
            program_id: npcs_treasury::ID,
            accounts: tacc::Redeem {
                treasury: self.treasury,
                vault: self.vault,
                signer: self.signer,
                asset_mint: self.asset,
                usdc_mint: self.env.mint,
                authority: who.pubkey(),
                authority_asset: *asset,
                authority_usdc: *usdc,
                token_program: self.env.tp.id(),
                asset_token_program: Tp::T22.id(),
                feed: feed_pda(&self.asset),
            }
            .to_account_metas(None),
            data: tix::Redeem { amount }.data(),
        };
        self.env.send(&[ix], &[who])
    }

    fn supply(&self) -> u64 {
        let a = self.env.svm.get_account(&self.asset).unwrap();
        spl_token_2022::state::Mint::unpack_from_slice(&a.data[..82])
            .unwrap()
            .supply
    }

    fn reconcile(&mut self) -> Reconciled {
        let ix = Instruction {
            program_id: npcs_treasury::ID,
            accounts: tacc::Reconcile {
                treasury: self.treasury,
                vault: self.vault,
                asset_mint: self.asset,
                usdc_mint: self.env.mint,
            }
            .to_account_metas(None),
            data: tix::Reconcile {}.data(),
        };
        let a = self.env.authority.insecure_clone();
        let meta = self.env.send(&[ix], &[&a]).unwrap();
        meta.logs
            .iter()
            .filter_map(|l| l.strip_prefix("Program data: "))
            .filter_map(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .find(|d| d.starts_with(Reconciled::DISCRIMINATOR))
            .map(|d| Reconciled::deserialize(&mut &d[8..]).unwrap())
            .unwrap()
    }
}

#[test]
fn deposit_mints_one_to_one_and_redeem_burns_against_the_vault() {
    for tp in Tp::ALL {
        let mut f = fx(tp);
        let (who, usdc) = f.env.bidder(10 * UNIT);
        let asset = f
            .env
            .token_account_in(Tp::T22, &f.asset.clone(), &who.pubkey(), 0);
        f.deposit(&who, &usdc, &asset, 5 * UNIT).unwrap();
        assert_eq!(f.env.bal(&asset), 5 * UNIT);
        assert_eq!(f.env.bal(&f.vault), 5 * UNIT);
        assert_eq!(f.supply(), 5 * UNIT);
        assert!(f.reconcile().backed);

        // The issuer (treasury authority) burns NPCS it holds for USDC.
        let a = f.env.authority.insecure_clone();
        let mint = f.env.mint;
        let a_usdc = f.env.token_account(&mint, &a.pubkey(), 3 * UNIT);
        let a_asset = f
            .env
            .token_account_in(Tp::T22, &f.asset.clone(), &a.pubkey(), 0);
        f.deposit(&a, &a_usdc, &a_asset, 3 * UNIT).unwrap();
        // Not the authority: refused.
        assert_eq!(
            f.redeem(&who, &asset, &usdc, UNIT).unwrap_err(),
            terr(TreasuryError::NotAuthority)
        );
        f.redeem(&a, &a_asset, &a_usdc, 2 * UNIT).unwrap();
        assert_eq!(f.env.bal(&a_usdc), 2 * UNIT);
        assert_eq!((f.supply(), f.env.bal(&f.vault)), (6 * UNIT, 6 * UNIT));
        assert!(f.reconcile().backed, "{tp:?}");
    }
}

#[test]
fn pause_stops_deposits_only() {
    let mut f = fx(Tp::Spl);
    let a = f.env.authority.insecure_clone();
    let ix = Instruction {
        program_id: npcs_treasury::ID,
        accounts: tacc::Admin {
            treasury: f.treasury,
            authority: a.pubkey(),
        }
        .to_account_metas(None),
        data: tix::SetPaused { paused: true }.data(),
    };
    f.env.send(&[ix], &[&a]).unwrap();
    let (who, usdc) = f.env.bidder(UNIT);
    let asset = f
        .env
        .token_account_in(Tp::T22, &f.asset.clone(), &who.pubkey(), 0);
    assert_eq!(
        f.deposit(&who, &usdc, &asset, UNIT).unwrap_err(),
        terr(TreasuryError::Paused)
    );
}

#[test]
fn a_mint_from_anywhere_else_breaks_i1_and_stops_the_treasury() {
    let mut f = fx(Tp::Spl);
    let (who, usdc) = f.env.bidder(10 * UNIT);
    let asset = f
        .env
        .token_account_in(Tp::T22, &f.asset.clone(), &who.pubkey(), 0);
    f.deposit(&who, &usdc, &asset, UNIT).unwrap();
    // Simulate supply created outside the treasury.
    let mut acct = f.env.svm.get_account(&f.asset).unwrap();
    let mut m = spl_token_2022::state::Mint::unpack_from_slice(&acct.data[..82]).unwrap();
    m.supply += 1;
    m.pack_into_slice(&mut acct.data[..82]);
    f.env.svm.set_account(f.asset, acct).unwrap();
    assert!(!f.reconcile().backed);
    assert_eq!(
        f.deposit(&who, &usdc, &asset, UNIT).unwrap_err(),
        terr(TreasuryError::Unreconciled)
    );
}

fn redeemed(logs: &[String]) -> Redeemed {
    logs.iter()
        .filter_map(|l| l.strip_prefix("Program data: "))
        .filter_map(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .find(|d| d.starts_with(Redeemed::DISCRIMINATOR))
        .map(|d| Redeemed::deserialize(&mut &d[8..]).unwrap())
        .unwrap()
}

/// doc 09 Path 8 (#202): a redemption pays `burned × NAV_effective`, not
/// 1:1. Below 1.00 the difference (the NAV fee accrual) stays in the vault,
/// so every remaining unit is still backed. A Stale mark redeems nothing.
#[test]
fn redemption_pays_the_published_nav_and_never_on_a_stale_mark() {
    let mut f = fx(Tp::Spl);
    let a = f.env.authority.insecure_clone();
    let mint = f.env.mint;
    let a_usdc = f.env.token_account(&mint, &a.pubkey(), 4 * UNIT);
    let a_asset = f
        .env
        .token_account_in(Tp::T22, &f.asset.clone(), &a.pubkey(), 0);
    f.deposit(&a, &a_usdc, &a_asset, 4 * UNIT).unwrap();
    // A week later the mark is 0.98.
    let (asset, publisher) = (f.asset, f.publisher.insecure_clone());
    f.env.set_time(f.env.now() + WEEK);
    f.env
        .publish_nav(&asset, &publisher, NAV_ONE * 98 / 100)
        .unwrap();
    let meta = f.redeem(&a, &a_asset, &a_usdc, 2 * UNIT).unwrap();
    let r = redeemed(&meta.logs);
    assert_eq!((r.burned, r.usdc), (2 * UNIT, 1_960_000));
    assert_eq!(f.env.bal(&a_usdc), 1_960_000);
    assert_eq!((f.supply(), f.env.bal(&f.vault)), (2 * UNIT, 2_040_000));
    assert!(f.reconcile().backed, "the 0.04 residue stays behind");
    // 15 days without a mark: Stale, no redemption at a stale price.
    f.env.set_time(f.env.now() + 15 * 86_400);
    assert_eq!(
        f.redeem(&a, &a_asset, &a_usdc, UNIT).unwrap_err(),
        terr(TreasuryError::MarkUnavailable)
    );
}

/// doc 09 I3.2 (#202): with the gate accounts appended, the units a
/// deposit mints are booked on the depositor's gate ledger as a
/// `PrimaryMint` lot (seasoning from now), signed by the gate-config
/// authority; a ledger that is not the depositor's is refused.
#[test]
fn a_deposit_books_the_minted_units_as_a_primary_mint_lot() {
    let mut f = fx(Tp::Spl);
    f.env.asset_mint = f.asset;
    f.env.open_gate_market();
    let (who, usdc) = f.env.bidder(10 * UNIT);
    f.env.admit_as(&who.pubkey(), true);
    let other = f.env.funded();
    f.env.admit_as(&other.pubkey(), true);
    let asset = f
        .env
        .token_account_in(Tp::T22, &f.asset.clone(), &who.pubkey(), 0);
    let auth = f.env.authority.insecure_clone();
    let gate = |env: &Env, ledger_of: &Pubkey| -> Vec<AccountMeta> {
        vec![
            AccountMeta::new_readonly(credit_gate::ID, false),
            AccountMeta::new_readonly(env.gate_config(), false),
            AccountMeta::new(env.lots_pda(ledger_of), false),
            AccountMeta::new_readonly(env.block_entry(ledger_of), false),
            AccountMeta::new_readonly(auth.pubkey(), true),
        ]
    };
    let mut ix = f.deposit_ix(&who, &usdc, &asset, 3 * UNIT);
    ix.accounts.extend(gate(&f.env, &other.pubkey()));
    assert_eq!(
        f.env.send(&[ix], &[&who, &auth]).unwrap_err(),
        terr(TreasuryError::GateAccounts)
    );
    let mut ix = f.deposit_ix(&who, &usdc, &asset, 3 * UNIT);
    ix.accounts.extend(gate(&f.env, &who.pubkey()));
    f.env.send(&[ix], &[&who, &auth]).expect("deposit + lot");
    let lots = f.env.lots(&who.pubkey());
    assert_eq!(lots.lot_count, 1);
    let lot = lots.lots[0];
    assert_eq!((lot.qty, lot.source), (3 * UNIT, LotSource::PrimaryMint));
    assert_eq!(lot.acquired_ts, f.env.now());
    assert_eq!(f.env.bal(&asset), 3 * UNIT);
}

/// doc 09 Path 5 (#202): `redeem_from` burns units another holder owns
/// (queue-claim's request vault; here a plain wallet consenting) with the
/// issuer co-signing, and pays the NAV value to the named destination.
#[test]
fn redeem_from_burns_a_consenting_holder_and_pays_the_destination() {
    let mut f = fx(Tp::Spl);
    let (holder, usdc) = f.env.bidder(5 * UNIT);
    let holder_asset = f
        .env
        .token_account_in(Tp::T22, &f.asset.clone(), &holder.pubkey(), 0);
    f.deposit(&holder, &usdc, &holder_asset, 5 * UNIT).unwrap();
    let mint = f.env.mint;
    let dest_owner = Pubkey::new_unique();
    let dest = f.env.token_account(&mint, &dest_owner, 0);
    let auth = f.env.authority.insecure_clone();
    let (treasury, vault, signer, asset, tp) =
        (f.treasury, f.vault, f.signer, f.asset, f.env.tp.id());
    let ix = |authority: &Pubkey| Instruction {
        program_id: npcs_treasury::ID,
        accounts: tacc::RedeemFrom {
            treasury,
            vault,
            signer,
            asset_mint: asset,
            usdc_mint: mint,
            feed: feed_pda(&asset),
            authority: *authority,
            holder: holder.pubkey(),
            holder_asset,
            usdc_destination: dest,
            token_program: tp,
            asset_token_program: Tp::T22.id(),
        }
        .to_account_metas(None),
        data: tix::RedeemFrom { amount: 2 * UNIT }.data(),
    };
    let stranger = f.env.funded();
    assert_eq!(
        f.env
            .send(&[ix(&stranger.pubkey())], &[&holder, &stranger])
            .unwrap_err(),
        terr(TreasuryError::NotAuthority)
    );
    f.env
        .send(&[ix(&auth.pubkey())], &[&holder, &auth])
        .expect("redeem_from");
    assert_eq!(f.env.bal(&dest), 2 * UNIT);
    assert_eq!(f.env.bal(&holder_asset), 3 * UNIT);
    assert!(f.reconcile().backed);
}
