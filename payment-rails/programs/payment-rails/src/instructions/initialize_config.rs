//! `initialize_config` — one-time creation of the global `RailsConfig`.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::error::RailsError;
use crate::events::{ConfigChange, RailsConfigUpdated};
use crate::program::PaymentRails;
use crate::state::RailsConfig;
use crate::USDC_DECIMALS;

pub fn handler(ctx: Context<InitializeConfig>) -> Result<()> {
    // Extract keys before the mutable account borrow (borrow discipline).
    let authority_key = ctx.accounts.authority.key();
    let config = RailsConfig {
        authority: authority_key,
        usdc_mint: ctx.accounts.usdc_mint.key(),
        fee_receiver: ctx.accounts.fee_receiver.key(),
        reserve: ctx.accounts.reserve.key(),
        // Authority operates sessions until the auction engine's signer PDA
        // is wired in via `set_operator`.
        operator: authority_key,
        // Start halted-clear: R1+ instructions consult `ensure_accepting`.
        paused: false,
        depeg_breaker: false,
        bump: ctx.bumps.config,
    };
    let new_config_hash = config.fingerprint();
    ctx.accounts.config.set_inner(config);
    emit!(RailsConfigUpdated {
        change: ConfigChange::Initialized,
        new_config_hash,
        authority: authority_key,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    /// Global rails config PDA.
    #[account(
        init,
        payer = authority,
        space = 8 + RailsConfig::INIT_SPACE,
        seeds = [RailsConfig::SEED],
        bump
    )]
    pub config: Account<'info, RailsConfig>,
    /// Settlement mint (USDC). Owner-checked against `token_program`.
    #[account(
        mint::token_program = token_program,
        constraint = usdc_mint.decimals == USDC_DECIMALS @ RailsError::InvalidMintDecimals,
    )]
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    /// Protocol fee token account, denominated in the settlement mint.
    #[account(
        constraint = fee_receiver.mint == usdc_mint.key() @ RailsError::MintMismatch,
        token::token_program = token_program,
    )]
    pub fee_receiver: InterfaceAccount<'info, TokenAccount>,
    /// Reserve token account (forfeited bonds, reserve cut), distinct from
    /// the fee receiver so the ledger can reconcile them separately.
    #[account(
        constraint = reserve.mint == usdc_mint.key() @ RailsError::MintMismatch,
        token::token_program = token_program,
        constraint = reserve.key() != fee_receiver.key() @ RailsError::DuplicateTreasuryAccount,
    )]
    pub reserve: InterfaceAccount<'info, TokenAccount>,
    /// Upgrade authority (Squads 2-of-2); becomes the config authority and
    /// pays rent.
    #[account(mut)]
    pub authority: Signer<'info>,
    /// This program — used only to locate its ProgramData account.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, PaymentRails>,
    /// Holds the upgrade authority the signer must match.
    #[account(
        constraint = program_data.upgrade_authority_address == Some(authority.key())
            @ RailsError::NotUpgradeAuthority
    )]
    pub program_data: Account<'info, ProgramData>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}
