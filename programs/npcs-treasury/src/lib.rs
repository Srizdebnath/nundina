//! `npcs-treasury` — the NPCS backing, enforced by code (#148).
//!
//! doc 09 §3 invariant **I1**: every NPCS base unit maps to a USDC that
//! entered the treasury, 1:1 at deposit; redemptions exit at the published
//! mark ("burn amount × published NAV is paid out of the sleeve"). SRS 08
//! §3.1 "backup mode": the USDC sits in a program-controlled treasury PDA
//! and NAV accrues flat. No yield venue is integrated (Kamino stays a
//! future venue, unnamed in `token/params.toml`).
//!
//! - The treasury's signer PDA must be the asset mint's **mint authority**
//!   (checked at `initialize`), so `deposit` is the only way NPCS is ever
//!   minted.
//! - `deposit(amount)`: USDC → the treasury vault, the same amount of NPCS
//!   (scaled by decimals) → the depositor's account (doc 09 Path 1: deposit
//!   stays 1:1). For default-frozen NPCS that account must be gate-thawed
//!   first; the token program refuses a frozen destination. With the gate
//!   accounts appended (and the gate-config authority co-signing: credit-gate
//!   `append_lot` is gate-authority-only) the minted units get their
//!   `PrimaryMint` seasoning lot in the same instruction (doc 09 §3 I3.2).
//! - `redeem(amount)`: the issuer authority burns NPCS it holds and takes
//!   `amount × NAV_effective` USDC out (doc 09 Path 8), NAV read from the
//!   asset's mark-engine feed, which must be Fresh or Warned (a Stale or
//!   Suspended mark redeems nothing).
//! - `redeem_from(amount)`: the same, burning from a holder other than the
//!   authority — queue-claim's request vault (doc 09 Path 5: the claim's
//!   NPCS burns at the window, the proceeds land in the claim's cash
//!   vault). The issuer authority co-signs every redemption.
//! - Every mint and burn checks I1 **before and after**: `supply ≤ vault ×
//!   scale` (every unit backed). A redemption below 1.00 leaves the NAV fee
//!   accrual in the vault, so the vault may hold more than the supply; a
//!   mint outside this program (or a vault drained by a delegate) leaves
//!   units unbacked and makes every further mint and burn fail with
//!   `Unreconciled` until governance repairs it; `reconcile`
//!   (permissionless) reports it.

#![deny(unsafe_code)]
#![allow(missing_docs)]
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke;
use anchor_lang::InstructionData;
use anchor_spl::token_interface::{
    self, Burn, Mint, MintTo, TokenAccount, TokenInterface, TransferChecked,
};
use mark_engine::Feed;

declare_id!("4BszRuAP4Gp7vdyu9MjE28gV8YTNtJZSRJRMQHdMut1y");

pub const TREASURY_SEED: &[u8] = b"treasury";
pub const VAULT_SEED: &[u8] = b"vault";
pub const SIGNER_SEED: &[u8] = b"treasury_signer";

pub fn signer_pda(treasury: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SIGNER_SEED, treasury.as_ref()], &crate::ID)
}

/// The treasury PDA of `asset_mint`.
pub fn treasury_pda(asset_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[TREASURY_SEED, asset_mint.as_ref()], &crate::ID)
}

/// Gate accounts appended to `deposit` for the I1 lot: credit-gate program,
/// market config, the depositor's `HolderLots`, its block entry, and the
/// gate-config authority (signer).
pub const GATE_LOT_ACCOUNTS: usize = 5;

#[program]
pub mod npcs_treasury {
    use super::*;

    /// Create the treasury for `asset_mint`. The asset's mint authority
    /// must already be this treasury's signer PDA (hand-over is a
    /// governance step on the mint), so no other mint path exists.
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let key = ctx.accounts.treasury.key();
        let (signer, signer_bump) = signer_pda(&key);
        require!(
            ctx.accounts.asset_mint.mint_authority == Some(signer).into(),
            TreasuryError::NotMintAuthority
        );
        require!(
            ctx.accounts.asset_mint.decimals >= ctx.accounts.usdc_mint.decimals,
            TreasuryError::BadDecimals
        );
        let t = &mut ctx.accounts.treasury;
        t.authority = ctx.accounts.authority.key();
        t.asset_mint = ctx.accounts.asset_mint.key();
        t.usdc_mint = ctx.accounts.usdc_mint.key();
        t.vault = ctx.accounts.vault.key();
        t.bump = ctx.bumps.treasury;
        t.signer_bump = signer_bump;
        emit!(TreasuryInitialized {
            treasury: key,
            asset_mint: t.asset_mint,
            vault: t.vault,
            authority: t.authority,
        });
        Ok(())
    }

    /// I1 mint-on-deposit: `amount` USDC in, the same value of NPCS out.
    /// Remaining accounts (optional, all or none): [`GATE_LOT_ACCOUNTS`] —
    /// the minted units are appended to the depositor's gate lot ledger.
    pub fn deposit<'info>(ctx: Context<'info, Deposit<'info>>, amount: u64) -> Result<()> {
        require!(amount > 0, TreasuryError::ZeroAmount);
        let t = &ctx.accounts.treasury;
        require!(!t.paused, TreasuryError::Paused);
        let scale = scale(&ctx.accounts.asset_mint, &ctx.accounts.usdc_mint);
        check_backed(&ctx.accounts.asset_mint, &ctx.accounts.vault, scale)?;
        let minted = amount
            .checked_mul(scale)
            .ok_or(TreasuryError::MathOverflow)?;
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.depositor_usdc.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.depositor.to_account_info(),
                },
            ),
            amount,
            ctx.accounts.usdc_mint.decimals,
        )?;
        let key = t.key();
        let seeds: &[&[u8]] = &[SIGNER_SEED, key.as_ref(), &[t.signer_bump]];
        token_interface::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.asset_token_program.key(),
                MintTo {
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.depositor_asset.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ),
            minted,
        )?;
        ctx.accounts.asset_mint.reload()?;
        ctx.accounts.vault.reload()?;
        check_backed(&ctx.accounts.asset_mint, &ctx.accounts.vault, scale)?;
        let gate = ctx.remaining_accounts;
        if !gate.is_empty() {
            append_mint_lot(
                gate,
                &ctx.accounts.asset_mint.to_account_info(),
                &ctx.accounts.depositor.key(),
                minted,
            )?;
        }
        let t = &mut ctx.accounts.treasury;
        t.total_deposited = t.total_deposited.saturating_add(amount);
        emit!(Deposited {
            treasury: key,
            depositor: ctx.accounts.depositor.key(),
            usdc: amount,
            minted,
            supply: ctx.accounts.asset_mint.supply,
            vault: ctx.accounts.vault.amount,
        });
        Ok(())
    }

    /// doc 09 Path 8: the issuer burns `amount` NPCS it holds and takes
    /// `amount × NAV_effective` USDC out of the vault (I1 preserved: every
    /// remaining unit stays backed). Authority only.
    pub fn redeem(ctx: Context<Redeem>, amount: u64) -> Result<()> {
        let a = &mut *ctx.accounts;
        let r = RedeemAccounts {
            treasury: &a.treasury,
            vault: &mut a.vault,
            signer: &a.signer,
            asset_mint: &mut a.asset_mint,
            usdc_mint: &a.usdc_mint,
            feed: &a.feed,
            burner: a.authority.to_account_info(),
            from: &a.authority_asset,
            to: &a.authority_usdc,
            token_program: &a.token_program,
            asset_token_program: &a.asset_token_program,
        };
        let (burned, usdc) = redeem_at_nav(r, amount)?;
        let t = &mut ctx.accounts.treasury;
        t.total_redeemed = t.total_redeemed.saturating_add(usdc);
        emit_redeemed(
            &ctx.accounts.treasury,
            burned,
            usdc,
            &ctx.accounts.asset_mint,
            &ctx.accounts.vault,
        );
        Ok(())
    }

    /// doc 09 Path 5/8: burn `amount` NPCS held by `holder` (queue-claim's
    /// request vault, its signer PDA signing) and pay `amount ×
    /// NAV_effective` USDC to `usdc_destination` (the claim's cash vault).
    /// The issuer authority co-signs: a redemption is always the issuer's
    /// act, the holder only consents to the burn.
    pub fn redeem_from(ctx: Context<RedeemFrom>, amount: u64) -> Result<()> {
        let a = &mut *ctx.accounts;
        let r = RedeemAccounts {
            treasury: &a.treasury,
            vault: &mut a.vault,
            signer: &a.signer,
            asset_mint: &mut a.asset_mint,
            usdc_mint: &a.usdc_mint,
            feed: &a.feed,
            burner: a.holder.to_account_info(),
            from: &a.holder_asset,
            to: &a.usdc_destination,
            token_program: &a.token_program,
            asset_token_program: &a.asset_token_program,
        };
        let (burned, usdc) = redeem_at_nav(r, amount)?;
        let t = &mut ctx.accounts.treasury;
        t.total_redeemed = t.total_redeemed.saturating_add(usdc);
        emit_redeemed(
            &ctx.accounts.treasury,
            burned,
            usdc,
            &ctx.accounts.asset_mint,
            &ctx.accounts.vault,
        );
        Ok(())
    }

    /// Report I1 (permissionless): supply vs vault, evented.
    pub fn reconcile(ctx: Context<Reconcile>) -> Result<()> {
        let scale = scale(&ctx.accounts.asset_mint, &ctx.accounts.usdc_mint);
        let supply = ctx.accounts.asset_mint.supply;
        let vault = ctx.accounts.vault.amount;
        emit!(Reconciled {
            treasury: ctx.accounts.treasury.key(),
            supply,
            vault,
            backed: is_backed(supply, vault, scale),
        });
        Ok(())
    }

    /// Stop deposits (authority). Redemptions stay open.
    pub fn set_paused(ctx: Context<Admin>, paused: bool) -> Result<()> {
        ctx.accounts.treasury.paused = paused;
        emit!(PauseChanged {
            treasury: ctx.accounts.treasury.key(),
            paused,
        });
        Ok(())
    }
}

/// NPCS base units per USDC micro (10^(asset_dec − usdc_dec)).
fn scale(asset: &InterfaceAccount<Mint>, usdc: &InterfaceAccount<Mint>) -> u64 {
    10u64.pow(u32::from(asset.decimals - usdc.decimals))
}

/// I1: every unit backed, `supply ≤ vault × scale`.
pub fn is_backed(supply: u64, vault: u64, scale: u64) -> bool {
    u128::from(supply) <= u128::from(vault) * u128::from(scale)
}

fn check_backed(
    asset: &InterfaceAccount<Mint>,
    vault: &InterfaceAccount<TokenAccount>,
    scale: u64,
) -> Result<()> {
    require!(
        is_backed(asset.supply, vault.amount, scale),
        TreasuryError::Unreconciled
    );
    Ok(())
}

/// USDC micros paid for `burned` base units at `nav_pico` (micro-USDC per
/// base unit × 10¹², mark-engine's scale), floored: the redeemer
/// initiates, the remaining holders keep the rounding.
pub fn payout_at_nav(burned: u64, nav_pico: u128) -> Option<u64> {
    let v = credit_math::mul_div(u128::from(burned), nav_pico, mark_engine::NAV_SCALE).ok()?;
    u64::try_from(v).ok()
}

/// The NAV a redemption pays at `now`: `NAV_effective` of a Fresh or
/// Warned mark (doc 09 §3 I2; ME-P3). Stale, Suspended or never-published
/// marks redeem nothing.
pub fn redemption_nav(feed: &Feed, now: i64) -> Result<u128> {
    require!(feed.allows_liquidation(now), TreasuryError::MarkUnavailable);
    let nav = feed.nav_effective(now)?;
    require!(nav > 0, TreasuryError::MarkUnavailable);
    Ok(nav)
}

struct RedeemAccounts<'a, 'info> {
    treasury: &'a Account<'info, Treasury>,
    vault: &'a mut Box<InterfaceAccount<'info, TokenAccount>>,
    signer: &'a UncheckedAccount<'info>,
    asset_mint: &'a mut Box<InterfaceAccount<'info, Mint>>,
    usdc_mint: &'a InterfaceAccount<'info, Mint>,
    feed: &'a Account<'info, Feed>,
    burner: AccountInfo<'info>,
    from: &'a InterfaceAccount<'info, TokenAccount>,
    to: &'a InterfaceAccount<'info, TokenAccount>,
    token_program: &'a Interface<'info, TokenInterface>,
    asset_token_program: &'a Interface<'info, TokenInterface>,
}

/// Burn `amount`, pay it out at the redemption NAV; I1 before and after.
fn redeem_at_nav(r: RedeemAccounts, amount: u64) -> Result<(u64, u64)> {
    require!(amount > 0, TreasuryError::ZeroAmount);
    let now = Clock::get()?.unix_timestamp;
    let nav = redemption_nav(r.feed, now)?;
    let usdc = payout_at_nav(amount, nav).ok_or(TreasuryError::MathOverflow)?;
    require!(usdc > 0, TreasuryError::ZeroAmount);
    let scale = scale(r.asset_mint, r.usdc_mint);
    check_backed(r.asset_mint, r.vault, scale)?;
    token_interface::burn(
        CpiContext::new(
            r.asset_token_program.key(),
            Burn {
                mint: r.asset_mint.to_account_info(),
                from: r.from.to_account_info(),
                authority: r.burner,
            },
        ),
        amount,
    )?;
    let key = r.treasury.key();
    let seeds: &[&[u8]] = &[SIGNER_SEED, key.as_ref(), &[r.treasury.signer_bump]];
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            r.token_program.key(),
            TransferChecked {
                from: r.vault.to_account_info(),
                mint: r.usdc_mint.to_account_info(),
                to: r.to.to_account_info(),
                authority: r.signer.to_account_info(),
            },
            &[seeds],
        ),
        usdc,
        r.usdc_mint.decimals,
    )?;
    r.asset_mint.reload()?;
    r.vault.reload()?;
    check_backed(r.asset_mint, r.vault, scale)?;
    Ok((amount, usdc))
}

fn emit_redeemed(
    t: &Account<Treasury>,
    burned: u64,
    usdc: u64,
    asset: &InterfaceAccount<Mint>,
    vault: &InterfaceAccount<TokenAccount>,
) {
    emit!(Redeemed {
        treasury: t.key(),
        burned,
        usdc,
        supply: asset.supply,
        vault: vault.amount,
    });
}

/// doc 09 I3.2: the minted units become a `PrimaryMint` lot on the
/// depositor's gate ledger. credit-gate `append_lot` is gate-config-authority
/// only, so that authority co-signs the deposit (it is passed through);
/// the ledger must be the depositor's own.
fn append_mint_lot<'info>(
    gate: &[AccountInfo<'info>],
    mint: &AccountInfo<'info>,
    depositor: &Pubkey,
    qty: u64,
) -> Result<()> {
    require!(gate.len() == GATE_LOT_ACCOUNTS, TreasuryError::GateAccounts);
    let (program, config, lots, block_entry, authority) =
        (&gate[0], &gate[1], &gate[2], &gate[3], &gate[4]);
    require_keys_eq!(program.key(), credit_gate::ID, TreasuryError::GateAccounts);
    let expected = Pubkey::find_program_address(
        &[
            credit_gate::HolderLots::SEED,
            mint.key.as_ref(),
            depositor.as_ref(),
        ],
        &credit_gate::ID,
    )
    .0;
    require_keys_eq!(lots.key(), expected, TreasuryError::GateAccounts);
    require!(authority.is_signer, TreasuryError::GateAccounts);
    let ix = Instruction {
        program_id: credit_gate::ID,
        accounts: vec![
            AccountMeta::new_readonly(config.key(), false),
            AccountMeta::new(lots.key(), false),
            AccountMeta::new_readonly(block_entry.key(), false),
            AccountMeta::new_readonly(mint.key(), false),
            AccountMeta::new_readonly(authority.key(), true),
        ],
        data: credit_gate::instruction::AppendLot {
            qty,
            source: credit_gate::LotSource::PrimaryMint,
            affiliate: false,
            exemption_path: 0,
        }
        .data(),
    };
    invoke(
        &ix,
        &[
            config.clone(),
            lots.clone(),
            block_entry.clone(),
            mint.clone(),
            authority.clone(),
            program.clone(),
        ],
    )
    .map_err(Into::into)
}

#[account]
#[derive(InitSpace)]
pub struct Treasury {
    pub authority: Pubkey,
    pub asset_mint: Pubkey,
    pub usdc_mint: Pubkey,
    pub vault: Pubkey,
    pub total_deposited: u64,
    pub total_redeemed: u64,
    pub paused: bool,
    pub bump: u8,
    pub signer_bump: u8,
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Treasury::INIT_SPACE,
        seeds = [TREASURY_SEED, asset_mint.key().as_ref()],
        bump
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        init,
        payer = authority,
        seeds = [VAULT_SEED, treasury.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = signer,
        token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the treasury's signer PDA (mint authority, vault owner).
    #[account(seeds = [SIGNER_SEED, treasury.key().as_ref()], bump)]
    pub signer: UncheckedAccount<'info>,
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds = [TREASURY_SEED, asset_mint.key().as_ref()],
        bump = treasury.bump,
        has_one = vault @ TreasuryError::Mismatch,
        has_one = asset_mint @ TreasuryError::Mismatch,
        has_one = usdc_mint @ TreasuryError::Mismatch
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the treasury's signer PDA (mint authority).
    #[account(seeds = [SIGNER_SEED, treasury.key().as_ref()], bump = treasury.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub depositor: Signer<'info>,
    #[account(mut, token::mint = usdc_mint, token::authority = depositor)]
    pub depositor_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = asset_mint, token::authority = depositor)]
    pub depositor_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Redeem<'info> {
    #[account(
        mut,
        seeds = [TREASURY_SEED, asset_mint.key().as_ref()],
        bump = treasury.bump,
        has_one = vault @ TreasuryError::Mismatch,
        has_one = asset_mint @ TreasuryError::Mismatch,
        has_one = usdc_mint @ TreasuryError::Mismatch,
        has_one = authority @ TreasuryError::NotAuthority
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the treasury's signer PDA (vault owner).
    #[account(seeds = [SIGNER_SEED, treasury.key().as_ref()], bump = treasury.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub authority: Signer<'info>,
    #[account(mut, token::mint = asset_mint, token::authority = authority)]
    pub authority_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = usdc_mint)]
    pub authority_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    /// The asset's mark-engine feed: the redemption NAV (doc 09 Path 8).
    #[account(
        seeds = [Feed::SEED, asset_mint.key().as_ref()],
        bump = feed.bump,
        seeds::program = mark_engine::ID
    )]
    pub feed: Box<Account<'info, Feed>>,
}

#[derive(Accounts)]
pub struct RedeemFrom<'info> {
    #[account(
        mut,
        seeds = [TREASURY_SEED, asset_mint.key().as_ref()],
        bump = treasury.bump,
        has_one = vault @ TreasuryError::Mismatch,
        has_one = asset_mint @ TreasuryError::Mismatch,
        has_one = usdc_mint @ TreasuryError::Mismatch,
        has_one = authority @ TreasuryError::NotAuthority
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the treasury's signer PDA (vault owner).
    #[account(seeds = [SIGNER_SEED, treasury.key().as_ref()], bump = treasury.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    /// The asset's mark-engine feed: the redemption NAV.
    #[account(
        seeds = [Feed::SEED, asset_mint.key().as_ref()],
        bump = feed.bump,
        seeds::program = mark_engine::ID
    )]
    pub feed: Box<Account<'info, Feed>>,
    /// The issuer: every redemption is its act.
    pub authority: Signer<'info>,
    /// Owner of the burned units (e.g. queue-claim's signer PDA).
    pub holder: Signer<'info>,
    #[account(mut, token::mint = asset_mint, token::authority = holder)]
    pub holder_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = usdc_mint)]
    pub usdc_destination: Box<InterfaceAccount<'info, TokenAccount>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Reconcile<'info> {
    #[account(has_one = vault @ TreasuryError::Mismatch, has_one = asset_mint @ TreasuryError::Mismatch, has_one = usdc_mint @ TreasuryError::Mismatch)]
    pub treasury: Box<Account<'info, Treasury>>,
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
}

#[derive(Accounts)]
pub struct Admin<'info> {
    #[account(mut, has_one = authority @ TreasuryError::NotAuthority)]
    pub treasury: Box<Account<'info, Treasury>>,
    pub authority: Signer<'info>,
}

#[event]
pub struct TreasuryInitialized {
    pub treasury: Pubkey,
    pub asset_mint: Pubkey,
    pub vault: Pubkey,
    pub authority: Pubkey,
}

#[event]
pub struct Deposited {
    pub treasury: Pubkey,
    pub depositor: Pubkey,
    pub usdc: u64,
    pub minted: u64,
    pub supply: u64,
    pub vault: u64,
}

#[event]
pub struct Redeemed {
    pub treasury: Pubkey,
    pub burned: u64,
    pub usdc: u64,
    pub supply: u64,
    pub vault: u64,
}

#[event]
pub struct Reconciled {
    pub treasury: Pubkey,
    pub supply: u64,
    pub vault: u64,
    pub backed: bool,
}

#[event]
pub struct PauseChanged {
    pub treasury: Pubkey,
    pub paused: bool,
}

#[error_code]
pub enum TreasuryError {
    #[msg("the treasury signer PDA is not the asset's mint authority")]
    NotMintAuthority,
    #[msg("asset decimals below settlement decimals, or a non-whole redemption")]
    BadDecimals,
    #[msg("amount must be positive")]
    ZeroAmount,
    #[msg("deposits are paused")]
    Paused,
    #[msg("I1 broken: supply != vault USDC")]
    Unreconciled,
    #[msg("account does not match the treasury")]
    Mismatch,
    #[msg("signer is not the treasury authority")]
    NotAuthority,
    #[msg("arithmetic overflow")]
    MathOverflow,
    #[msg("no redeemable NAV: the mark is Stale, Suspended or never published")]
    MarkUnavailable,
    #[msg("gate lot accounts do not match the depositor")]
    GateAccounts,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAV_ONE: u128 = 1_000_000_000_000;

    #[test]
    fn payout_is_burn_times_nav_floored() {
        // 6dp asset / 6dp USDC: 1.000 NAV pays 1:1.
        assert_eq!(payout_at_nav(5_000_000, NAV_ONE), Some(5_000_000));
        // NAV 0.999980821918 (one week of 10bps fee): 1 NPCS → 999_980 µ.
        assert_eq!(payout_at_nav(1_000_000, 999_980_821_918), Some(999_980));
        // A sub-micro payout floors to zero (the redeemer initiates).
        assert_eq!(payout_at_nav(1, NAV_ONE - 1), Some(0));
        // 9dp asset at 1 USDC/NPCS: NAV per base unit is 10⁹ pico.
        assert_eq!(
            payout_at_nav(3_000_000_000, NAV_ONE / 1_000),
            Some(3_000_000)
        );
        assert_eq!(payout_at_nav(u64::MAX, u128::MAX), None);
    }

    #[test]
    fn backing_allows_surplus_not_shortfall() {
        assert!(is_backed(5, 5, 1));
        assert!(is_backed(4, 5, 1), "NAV fee residue stays in the vault");
        assert!(!is_backed(6, 5, 1), "an outside mint is unbacked");
        assert!(is_backed(5_000, 5, 1_000));
        assert!(!is_backed(5_001, 5, 1_000));
    }
}
