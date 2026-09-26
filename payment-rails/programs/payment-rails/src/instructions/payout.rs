//! Payout registry (R3, the "private" rails): attested wallets only, hashes
//! only. Every R3 payout path (facility advance, sweep remainder, coupons)
//! resolves its destination through `load_destination` + `ensure_payable`.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::TokenAccount;

use crate::error::RailsError;
use crate::events::{DestinationAction, DestinationEvent};
use crate::state::{PayoutDestination, RailsConfig};

/// Resolve a payout destination from a raw account, so that *every* way of
/// not being allowlisted — never registered, wrong program, garbage data,
/// an entry for someone else — fails with the typed
/// `DestinationNotAllowlisted` instead of a generic framework error.
///
/// `expected_owner`: when the payout is for a specific wallet (advance
/// borrower, sweep holder), the entry must be that wallet's.
pub(crate) fn load_destination(
    info: &AccountInfo,
    expected_owner: Option<&Pubkey>,
) -> Result<PayoutDestination> {
    let not_allowlisted = || error!(RailsError::DestinationNotAllowlisted);
    if info.owner != &crate::ID || info.data_is_empty() {
        return Err(not_allowlisted());
    }
    let dest = {
        let data = info.try_borrow_data()?;
        PayoutDestination::try_deserialize(&mut &data[..]).map_err(|_| not_allowlisted())?
    };
    // The account must be the canonical PDA of the owner it names.
    let pda = Pubkey::create_program_address(
        &[PayoutDestination::SEED, dest.owner.as_ref(), &[dest.bump]],
        &crate::ID,
    )
    .map_err(|_| not_allowlisted())?;
    require_keys_eq!(pda, *info.key, RailsError::DestinationNotAllowlisted);
    if let Some(owner) = expected_owner {
        require_keys_eq!(dest.owner, *owner, RailsError::DestinationNotAllowlisted);
    }
    Ok(dest)
}

/// Admit `owner` with `token_account` as its only payout account. The
/// registrar attests off-chain and records only the reference hash.
pub fn process_register_destination(
    ctx: Context<RegisterDestination>,
    attestation_ref_hash: [u8; 32],
    expires_at: i64,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require!(expires_at > now, RailsError::DestinationExpired);
    let dest = PayoutDestination {
        owner: ctx.accounts.owner.key(),
        token_account: ctx.accounts.token_account.key(),
        attestation_ref_hash,
        registrar: ctx.accounts.registrar.key(),
        expires_at,
        active: true,
        bump: ctx.bumps.destination,
    };
    emit_dest(DestinationAction::Registered, &dest);
    ctx.accounts.destination.set_inner(dest);
    Ok(())
}

/// Refresh the attestation (new hash / expiry), optionally re-point the
/// token account, and re-activate.
pub fn process_renew_destination(
    ctx: Context<RenewDestination>,
    attestation_ref_hash: [u8; 32],
    expires_at: i64,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require!(expires_at > now, RailsError::DestinationExpired);
    let registrar = ctx.accounts.registrar.key();
    let token_account = ctx.accounts.token_account.key();
    let dest = &mut ctx.accounts.destination;
    dest.token_account = token_account;
    dest.attestation_ref_hash = attestation_ref_hash;
    dest.expires_at = expires_at;
    dest.registrar = registrar;
    dest.active = true;
    emit_dest(DestinationAction::Renewed, dest);
    Ok(())
}

/// Revoke immediately (block-list precedence, CG-P5): every payout to this
/// wallet fails with `DestinationRevoked` until renewed.
pub fn process_revoke_destination(ctx: Context<RevokeDestination>) -> Result<()> {
    let registrar = ctx.accounts.registrar.key();
    let dest = &mut ctx.accounts.destination;
    dest.active = false;
    dest.registrar = registrar;
    emit_dest(DestinationAction::Revoked, dest);
    Ok(())
}

fn emit_dest(action: DestinationAction, d: &PayoutDestination) {
    emit!(DestinationEvent {
        action,
        owner: d.owner,
        token_account: d.token_account,
        attestation_ref_hash: d.attestation_ref_hash,
        expires_at: d.expires_at,
        registrar: d.registrar,
    });
}

#[derive(Accounts)]
pub struct RegisterDestination<'info> {
    #[account(
        seeds = [RailsConfig::SEED],
        bump = config.bump,
        has_one = registrar @ RailsError::NotRegistrar
    )]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        init,
        payer = registrar,
        space = 8 + PayoutDestination::INIT_SPACE,
        seeds = [PayoutDestination::SEED, owner.key().as_ref()],
        bump
    )]
    pub destination: Box<Account<'info, PayoutDestination>>,
    /// CHECK: any wallet; bound to `token_account` by the owner check below.
    pub owner: UncheckedAccount<'info>,
    /// The payout account: a settlement-mint account the owner controls.
    #[account(
        constraint = token_account.owner == owner.key() @ RailsError::NotEscrowOwner,
        constraint = token_account.mint == config.usdc_mint @ RailsError::MintMismatch,
    )]
    pub token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Payout registrar; pays rent.
    #[account(mut)]
    pub registrar: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RenewDestination<'info> {
    #[account(
        seeds = [RailsConfig::SEED],
        bump = config.bump,
        has_one = registrar @ RailsError::NotRegistrar
    )]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        mut,
        seeds = [PayoutDestination::SEED, destination.owner.as_ref()],
        bump = destination.bump
    )]
    pub destination: Box<Account<'info, PayoutDestination>>,
    #[account(
        constraint = token_account.owner == destination.owner @ RailsError::NotEscrowOwner,
        constraint = token_account.mint == config.usdc_mint @ RailsError::MintMismatch,
    )]
    pub token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    pub registrar: Signer<'info>,
}

#[derive(Accounts)]
pub struct RevokeDestination<'info> {
    #[account(
        seeds = [RailsConfig::SEED],
        bump = config.bump,
        has_one = registrar @ RailsError::NotRegistrar
    )]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        mut,
        seeds = [PayoutDestination::SEED, destination.owner.as_ref()],
        bump = destination.bump
    )]
    pub destination: Box<Account<'info, PayoutDestination>>,
    pub registrar: Signer<'info>,
}
