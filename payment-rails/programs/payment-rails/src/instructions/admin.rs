//! Governed config writes: operator hand-over and the two halt switches.
//! Authority-only (Squads 2-of-2), always evented.

use anchor_lang::prelude::*;

use crate::error::RailsError;
use crate::events::{Breaker, BreakerChanged, ConfigChange, RailsConfigUpdated};
use crate::state::RailsConfig;

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
