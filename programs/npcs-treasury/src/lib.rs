//! `npcs-treasury` — the NPCS backing, enforced by code (#148).
//!
//! doc 09 §3 invariant **I1**: NPCS supply equals the USDC held by the
//! treasury, 1:1 at deposit. SRS 08 §3.1 "backup mode": the USDC sits in a
//! program-controlled treasury PDA and NAV accrues flat. No yield venue is
//! integrated (Kamino stays a future venue, unnamed in `token/params.toml`).
//!
//! - The treasury's signer PDA must be the asset mint's **mint authority**
//!   (checked at `initialize`), so `deposit` is the only way NPCS is ever
//!   minted.
//! - `deposit(amount)`: USDC → the treasury vault, the same amount of NPCS
//!   (scaled by decimals) → the depositor's account. For default-frozen
//!   NPCS that account must be gate-thawed first; the token program refuses
//!   a frozen destination.
//! - `redeem(amount)`: the issuer authority burns NPCS it holds (e.g. what
//!   queue-claim `record_fill` delivered to it) and takes the same USDC out.
//! - Every mint and burn checks I1 **before and after**: `supply ==
//!   vault × scale`. A mint outside this program (or a vault drained by a
//!   delegate) makes every further mint and burn fail with `Unreconciled`
//!   until governance repairs it; `reconcile` (permissionless) reports it.

#![deny(unsafe_code)]
#![allow(missing_docs)]
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    self, Burn, Mint, MintTo, TokenAccount, TokenInterface, TransferChecked,
};

declare_id!("4BszRuAP4Gp7vdyu9MjE28gV8YTNtJZSRJRMQHdMut1y");

pub const TREASURY_SEED: &[u8] = b"treasury";
pub const VAULT_SEED: &[u8] = b"vault";
pub const SIGNER_SEED: &[u8] = b"treasury_signer";

pub fn signer_pda(treasury: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SIGNER_SEED, treasury.as_ref()], &crate::ID)
}

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
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(amount > 0, TreasuryError::ZeroAmount);
        let t = &ctx.accounts.treasury;
        require!(!t.paused, TreasuryError::Paused);
        let scale = scale(&ctx.accounts.asset_mint, &ctx.accounts.usdc_mint);
        check_i1(&ctx.accounts.asset_mint, &ctx.accounts.vault, scale)?;
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
        check_i1(&ctx.accounts.asset_mint, &ctx.accounts.vault, scale)?;
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

    /// The issuer burns `amount` NPCS it holds and takes the same USDC out
    /// of the vault (I1 preserved). Authority only.
    pub fn redeem(ctx: Context<Redeem>, amount: u64) -> Result<()> {
        require!(amount > 0, TreasuryError::ZeroAmount);
        let t = &ctx.accounts.treasury;
        let scale = scale(&ctx.accounts.asset_mint, &ctx.accounts.usdc_mint);
        require!(
            amount.checked_rem(scale) == Some(0),
            TreasuryError::BadDecimals
        );
        check_i1(&ctx.accounts.asset_mint, &ctx.accounts.vault, scale)?;
        token_interface::burn(
            CpiContext::new(
                ctx.accounts.asset_token_program.key(),
                Burn {
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    from: ctx.accounts.authority_asset.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
            ),
            amount,
        )?;
        let usdc = amount / scale;
        let key = t.key();
        let seeds: &[&[u8]] = &[SIGNER_SEED, key.as_ref(), &[t.signer_bump]];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.vault.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.authority_usdc.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ),
            usdc,
            ctx.accounts.usdc_mint.decimals,
        )?;
        ctx.accounts.asset_mint.reload()?;
        ctx.accounts.vault.reload()?;
        check_i1(&ctx.accounts.asset_mint, &ctx.accounts.vault, scale)?;
        let t = &mut ctx.accounts.treasury;
        t.total_redeemed = t.total_redeemed.saturating_add(usdc);
        emit!(Redeemed {
            treasury: key,
            burned: amount,
            usdc,
            supply: ctx.accounts.asset_mint.supply,
            vault: ctx.accounts.vault.amount,
        });
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
            backed: u128::from(supply) == u128::from(vault) * u128::from(scale),
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

/// I1: `supply == vault × scale`.
fn check_i1(
    asset: &InterfaceAccount<Mint>,
    vault: &InterfaceAccount<TokenAccount>,
    scale: u64,
) -> Result<()> {
    require!(
        u128::from(asset.supply) == u128::from(vault.amount) * u128::from(scale),
        TreasuryError::Unreconciled
    );
    Ok(())
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
}
