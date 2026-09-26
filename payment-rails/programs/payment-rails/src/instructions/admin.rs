//! Governed config writes: operator hand-over and the two halt switches.
//! Authority-only (Squads 2-of-2), always evented.

use anchor_lang::prelude::*;

use crate::error::RailsError;
use crate::events::{Breaker, BreakerChanged, ConfigChange, RailsConfigUpdated};
use crate::state::{RailsConfig, MAX_FEE_BPS, MAX_RESERVE_BPS};

pub fn process_set_operator(ctx: Context<AdminConfig>, operator: Pubkey) -> Result<()> {
    let authority_key = ctx.accounts.authority.key();
    let cfg = &mut ctx.accounts.config;
    cfg.operator = operator;
    emit!(RailsConfigUpdated {
        change: ConfigChange::Operator,
        new_config_hash: cfg.fingerprint(),
        authority: authority_key,
    });
    Ok(())
}

/// Re-point the payout registrar (R3). Existing destinations stay valid;
/// only the new key can admit, renew or revoke from now on.
pub fn process_set_registrar(ctx: Context<AdminConfig>, registrar: Pubkey) -> Result<()> {
    let authority_key = ctx.accounts.authority.key();
    let cfg = &mut ctx.accounts.config;
    cfg.registrar = registrar;
    emit!(RailsConfigUpdated {
        change: ConfigChange::Registrar,
        new_config_hash: cfg.fingerprint(),
        authority: authority_key,
    });
    Ok(())
}

/// Set the fee and reserve cut (bps of each fill's gross). Bounded by the
/// hard on-chain ceilings — governance can never exceed them. Sessions
/// snapshot these at open, so a change never re-prices a live session.
pub fn process_set_fees(ctx: Context<AdminConfig>, fee_bps: u16, reserve_bps: u16) -> Result<()> {
    require!(
        fee_bps <= MAX_FEE_BPS && reserve_bps <= MAX_RESERVE_BPS,
        RailsError::FeeTooHigh
    );
    let authority_key = ctx.accounts.authority.key();
    let cfg = &mut ctx.accounts.config;
    cfg.fee_bps = fee_bps;
    cfg.reserve_bps = reserve_bps;
    emit!(RailsConfigUpdated {
        change: ConfigChange::Fees,
        new_config_hash: cfg.fingerprint(),
        authority: authority_key,
    });
    Ok(())
}

/// Flip `paused` or `depeg_breaker`. Neither ever blocks a refund (AE-P10);
/// they only stop new money in and settlement.
pub fn process_set_breaker(
    ctx: Context<AdminConfig>,
    breaker: Breaker,
    engaged: bool,
    evidence_hash: [u8; 32],
) -> Result<()> {
    let authority_key = ctx.accounts.authority.key();
    let cfg = &mut ctx.accounts.config;
    match breaker {
        Breaker::Pause => cfg.paused = engaged,
        Breaker::Depeg => cfg.depeg_breaker = engaged,
    }
    emit!(BreakerChanged {
        breaker,
        engaged,
        evidence_hash,
        authority: authority_key,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct AdminConfig<'info> {
    /// Global rails config; only its authority may write.
    #[account(
        mut,
        seeds = [RailsConfig::SEED],
        bump = config.bump,
        has_one = authority @ RailsError::Unauthorized
    )]
    pub config: Account<'info, RailsConfig>,
    /// Squads 2-of-2 authority (checked by `has_one`).
    pub authority: Signer<'info>,
}
