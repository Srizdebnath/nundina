//! Cash-session lifecycle driven by the operator: open, report reveals,
//! fail. None of these move cash — the operator can never touch escrow.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::error::RailsError;
use crate::events::{EscrowAction, EscrowEvent, SessionAction, SessionEvent};
use crate::state::{CashSession, Escrow, RailsConfig, SessionStatus};

/// Operator-chosen session parameters, snapshotted at open (AE-P1, L5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub struct SessionParams {
    /// Deposits accepted while `now < commit_end`.
    pub commit_end: i64,
    /// Reveals reported until `reveal_end`; settlement opens at it.
    pub reveal_end: i64,
    /// Hard deadline: settlement closes, every escrow refundable.
    pub settle_deadline: i64,
    /// Bond per escrow (micro-USDC).
    pub bond_amount: u64,
    /// Per-investor cash cap (micro-USDC).
    pub max_cash: u64,
    /// Issuer ROFR authority (AE-P7); `Pubkey::default()` disables ROFR.
    pub rofr_authority: Pubkey,
}

/// AE-P1/P3: open a cash session, snapshotting everything settlement and
/// refunds depend on (mints, fee receiver, reserve, fee bps, deadlines,
/// bond, cap, ROFR authority). Immutable afterwards (L5).
pub fn process_open_session(
    ctx: Context<OpenSession>,
    session_id: u64,
    params: SessionParams,
) -> Result<()> {
    let cfg = &ctx.accounts.config;
    cfg.ensure_accepting()?;
    let now = Clock::get()?.unix_timestamp;
    let SessionParams {
        commit_end,
        reveal_end,
        settle_deadline,
        bond_amount,
        max_cash,
        rofr_authority,
    } = params;
    require!(
        now < commit_end && commit_end < reveal_end && reveal_end < settle_deadline,
        RailsError::InvalidSchedule
    );
    require!(bond_amount > 0 && max_cash > 0, RailsError::ZeroAmount);
    // The asset is never the settlement mint (no USDC-for-USDC sessions).
    require_keys_neq!(
        ctx.accounts.asset_mint.key(),
        cfg.usdc_mint,
        RailsError::AssetMismatch
    );

    let session_key = ctx.accounts.session.key();
    let operator_key = ctx.accounts.operator.key();
    ctx.accounts.session.set_inner(CashSession {
        operator: operator_key,
        session_id,
        usdc_mint: cfg.usdc_mint,
        reserve: cfg.reserve,
        fee_receiver: cfg.fee_receiver,
        asset_mint: ctx.accounts.asset_mint.key(),
        asset_decimals: ctx.accounts.asset_mint.decimals,
        fee_bps: cfg.fee_bps,
        reserve_bps: cfg.reserve_bps,
        rofr_authority,
        commit_end,
        reveal_end,
        settle_deadline,
        bond_amount,
        max_cash,
        status: SessionStatus::Open,
        failed_at: 0,
        open_escrows: 0,
        total_cash: 0,
        total_bonds: 0,
        total_asset: 0,
        fills_settled: 0,
        volume_settled: 0,
        gross_settled: 0,
        bump: ctx.bumps.session,
    });
    emit!(SessionEvent {
        action: SessionAction::Opened,
        session: session_key,
        operator: operator_key,
        settle_deadline,
        ts: now,
    });
    Ok(())
}

/// AE-P4: the operator (auction engine, after its preimage check) reports
/// that this escrow's bid was revealed. Only inside the reveal window.
pub fn process_mark_revealed(ctx: Context<OperatorEscrow>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let session = &ctx.accounts.session;
    require!(
        session.status == SessionStatus::Open,
        RailsError::SessionNotOpen
    );
    require!(
        now >= session.commit_end && now < session.reveal_end,
        RailsError::OutsideWindow
    );
    let escrow = &mut ctx.accounts.escrow;
    require!(!escrow.revealed, RailsError::AlreadyRevealed);
    escrow.revealed = true;
    emit!(EscrowEvent {
        action: EscrowAction::Revealed,
        session: session.key(),
        escrow: escrow.key(),
        owner: escrow.owner,
        amount: 0,
        caller: ctx.accounts.operator.key(),
    });
    Ok(())
}

/// AE-P2/P4: abort the session (insufficient commitments, no cross ≥
/// floor, …). Every escrow becomes refundable at once; bonds are forfeited
/// only if reveals had already closed (see `CashSession::bond_forfeit`).
pub fn process_fail_session(ctx: Context<OperatorSession>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let session_key = ctx.accounts.session.key();
    let session = &mut ctx.accounts.session;
    require!(
        session.status == SessionStatus::Open,
        RailsError::SessionNotOpen
    );
    session.status = SessionStatus::Failed;
    session.failed_at = now;
    emit!(SessionEvent {
        action: SessionAction::Failed,
        session: session_key,
        operator: session.operator,
        settle_deadline: session.settle_deadline,
        ts: now,
    });
    Ok(())
}

/// R2: the operator declares settlement complete. No further fills; every
/// remaining escrow (buyer excess, unsold asset, bonds) becomes refundable
/// at once instead of waiting for the hard deadline.
pub fn process_finish_session(ctx: Context<OperatorSession>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let session_key = ctx.accounts.session.key();
    let session = &mut ctx.accounts.session;
    require!(
        session.status == SessionStatus::Open,
        RailsError::SessionNotOpen
    );
    // Settlement only exists after reveals close.
    require!(now >= session.reveal_end, RailsError::OutsideWindow);
    session.status = SessionStatus::Settled;
    emit!(SessionEvent {
        action: SessionAction::Settled,
        session: session_key,
        operator: session.operator,
        settle_deadline: session.settle_deadline,
        ts: now,
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(session_id: u64)]
pub struct OpenSession<'info> {
    /// Global config: operator check + mint/reserve snapshot source.
    #[account(
        seeds = [RailsConfig::SEED],
        bump = config.bump,
        has_one = operator @ RailsError::NotOperator
    )]
    pub config: Account<'info, RailsConfig>,
    /// New session PDA.
    #[account(
        init,
        payer = operator,
        space = 8 + CashSession::INIT_SPACE,
        seeds = [CashSession::SEED, operator.key().as_ref(), &session_id.to_le_bytes()],
        bump
    )]
    pub session: Account<'info, CashSession>,
    /// Traded asset mint (any token program); snapshotted with its decimals.
    pub asset_mint: InterfaceAccount<'info, Mint>,
    /// Config operator; pays rent.
    #[account(mut)]
    pub operator: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct OperatorEscrow<'info> {
    /// Session; only its snapshotted operator may report on it.
    #[account(has_one = operator @ RailsError::NotOperator)]
    pub session: Account<'info, CashSession>,
    /// Escrow in that session.
    #[account(
        mut,
        seeds = [Escrow::SEED, session.key().as_ref(), escrow.owner.as_ref()],
        bump = escrow.bump,
        has_one = session
    )]
    pub escrow: Account<'info, Escrow>,
    pub operator: Signer<'info>,
}

#[derive(Accounts)]
pub struct OperatorSession<'info> {
    /// Session to abort / finish; only its snapshotted operator may.
    #[account(mut, has_one = operator @ RailsError::NotOperator)]
    pub session: Account<'info, CashSession>,
    pub operator: Signer<'info>,
}
