//! Cash-session lifecycle driven by the operator: open, report reveals,
//! fail. None of these move cash — the operator can never touch escrow.

use anchor_lang::prelude::*;

use crate::error::RailsError;
use crate::events::{EscrowAction, EscrowEvent, SessionAction, SessionEvent};
use crate::state::{CashSession, Escrow, RailsConfig, SessionStatus};

/// AE-P1/P3: open a cash session, snapshotting everything a refund depends
/// on (mint, reserve, deadlines, bond, cap). Immutable afterwards (L5).
pub fn process_open_session(
    ctx: Context<OpenSession>,
    session_id: u64,
    commit_end: i64,
    reveal_end: i64,
    settle_deadline: i64,
    bond_amount: u64,
    max_cash: u64,
) -> Result<()> {
    let cfg = &ctx.accounts.config;
    cfg.ensure_accepting()?;
    let now = Clock::get()?.unix_timestamp;
    require!(
        now < commit_end && commit_end < reveal_end && reveal_end < settle_deadline,
        RailsError::InvalidSchedule
    );
    require!(bond_amount > 0 && max_cash > 0, RailsError::ZeroAmount);

    let session_key = ctx.accounts.session.key();
    let operator_key = ctx.accounts.operator.key();
    ctx.accounts.session.set_inner(CashSession {
        operator: operator_key,
        session_id,
        usdc_mint: cfg.usdc_mint,
        reserve: cfg.reserve,
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
pub fn process_fail_session(ctx: Context<FailSession>) -> Result<()> {
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
pub struct FailSession<'info> {
    /// Session to abort; only its snapshotted operator may.
    #[account(mut, has_one = operator @ RailsError::NotOperator)]
    pub session: Account<'info, CashSession>,
    pub operator: Signer<'info>,
}
