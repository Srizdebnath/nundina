//! Bonded keepers with a permissionless fallback (doc 06 §8, SRS 08 §6,
//! #127).
//!
//! The crank-required step after a session's hard deadline is refunding
//! its escrows. Refunds were already permissionless (AE-P10); this adds the
//! accountability doc 06 §8 asks for:
//!
//! 1. A keeper posts a USDC bond (`register_keeper`, ≥ `min_bond`) into a
//!    vault owned by its `Keeper` PDA.
//! 2. The session operator assigns it (`assign_keeper`), snapshotting
//!    `due_at = settle_deadline + sla_seconds` and the escalation reward.
//! 3. Within the SLA the keeper refunds through the ordinary `refund` /
//!    `refund_ask`. After `due_at`, **anyone** may call `refund_escalated` /
//!    `refund_ask_escalated`: the same refund (one shared body), plus a
//!    reward from the keeper's bond to the caller, and a recorded miss.
//! 4. `release_assignment` (anyone) closes the duty once no escrow is left;
//!    only then can the keeper `withdraw_keeper` its bond.
//!
//! Liveness is never conditional on keeper accounting: an escalated refund
//! pays a reward only out of what the bond still holds (possibly zero) and
//! completes regardless.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::error::RailsError;
use crate::events::{KeeperAction, KeeperEvent};
// Glob: composite `Accounts` fields need the derive-generated client modules.
use crate::instructions::escrow::*;
use crate::instructions::token;
use crate::state::{CashSession, Keeper, KeeperPolicy, RailsConfig, SessionKeeper};

fn emit_keeper(action: KeeperAction, keeper: Pubkey, session: Pubkey, amount: u64, caller: Pubkey) {
    emit!(KeeperEvent {
        action,
        keeper,
        session,
        amount,
        caller,
    });
}

pub fn process_initialize_keeper_policy(
    ctx: Context<InitializeKeeperPolicy>,
    min_bond: u64,
    sla_seconds: i64,
    escalation_reward: u64,
) -> Result<()> {
    KeeperPolicy::check(min_bond, sla_seconds, escalation_reward)?;
    ctx.accounts.policy.set_inner(KeeperPolicy {
        min_bond,
        sla_seconds,
        escalation_reward,
        bump: ctx.bumps.policy,
    });
    Ok(())
}

/// Live assignments keep their snapshotted SLA and reward (L5).
pub fn process_set_keeper_policy(
    ctx: Context<SetKeeperPolicy>,
    min_bond: u64,
    sla_seconds: i64,
    escalation_reward: u64,
) -> Result<()> {
    KeeperPolicy::check(min_bond, sla_seconds, escalation_reward)?;
    let p = &mut ctx.accounts.policy;
    p.min_bond = min_bond;
    p.sla_seconds = sla_seconds;
    p.escalation_reward = escalation_reward;
    Ok(())
}

pub fn process_register_keeper(ctx: Context<RegisterKeeper>, bond: u64) -> Result<()> {
    require!(
        bond >= ctx.accounts.policy.min_bond,
        RailsError::KeeperBondTooLow
    );
    let a = &ctx.accounts;
    token::transfer(
        &a.token_program,
        &a.usdc_mint,
        a.keeper_token.to_account_info(),
        a.vault.to_account_info(),
        a.wallet.to_account_info(),
        &[],
        bond,
    )?;
    let (wallet, vault) = (a.wallet.key(), a.vault.key());
    ctx.accounts.keeper.set_inner(Keeper {
        wallet,
        vault,
        assignments: 0,
        missed: 0,
        bump: ctx.bumps.keeper,
    });
    emit_keeper(
        KeeperAction::Registered,
        wallet,
        Pubkey::default(),
        bond,
        wallet,
    );
    Ok(())
}

pub fn process_top_up_keeper(ctx: Context<TopUpKeeper>, amount: u64) -> Result<()> {
    let a = &ctx.accounts;
    token::transfer(
        &a.token_program,
        &a.usdc_mint,
        a.keeper_token.to_account_info(),
        a.vault.to_account_info(),
        a.wallet.to_account_info(),
        &[],
        amount,
    )?;
    emit_keeper(
        KeeperAction::ToppedUp,
        a.wallet.key(),
        Pubkey::default(),
        amount,
        a.wallet.key(),
    );
    Ok(())
}

pub fn process_assign_keeper(ctx: Context<AssignKeeper>) -> Result<()> {
    let bumps = ctx.bumps;
    let a = ctx.accounts;
    // The bond must cover the policy minimum at assignment, not just at
    // registration: escalations may have drawn it down since.
    require!(
        a.vault.amount >= a.policy.min_bond,
        RailsError::KeeperBondTooLow
    );
    let due_at = a
        .session
        .settle_deadline
        .checked_add(a.policy.sla_seconds)
        .ok_or(RailsError::MathOverflow)?;
    a.keeper.assignments = a
        .keeper
        .assignments
        .checked_add(1)
        .ok_or(RailsError::MathOverflow)?;
    a.session_keeper.set_inner(SessionKeeper {
        session: a.session.key(),
        keeper: a.keeper.key(),
        due_at,
        reward: a.policy.escalation_reward,
        escalations: 0,
        bump: bumps.session_keeper,
    });
    emit_keeper(
        KeeperAction::Assigned,
        a.keeper.wallet,
        a.session.key(),
        0,
        a.operator.key(),
    );
    Ok(())
}

/// The reward leg of an escalation: up to the snapshotted reward, out of
/// whatever the bond still holds. Never fails for lack of bond.
fn pay_escalation<'info>(
    k: &mut KeeperAccounts<'info>,
    session: Pubkey,
    caller: Pubkey,
    token_program: &Interface<'info, TokenInterface>,
    usdc_mint: &InterfaceAccount<'info, Mint>,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require!(now >= k.session_keeper.due_at, RailsError::KeeperSlaNotDue);
    let reward = k.session_keeper.reward.min(k.vault.amount);
    let wallet = k.keeper.wallet;
    let seeds: &[&[u8]] = &[Keeper::SEED, wallet.as_ref(), &[k.keeper.bump]];
    token::transfer(
        token_program,
        usdc_mint,
        k.vault.to_account_info(),
        k.caller_token.to_account_info(),
        k.keeper.to_account_info(),
        &[seeds],
        reward,
    )?;
    k.keeper.missed = k.keeper.missed.saturating_add(1);
    k.session_keeper.escalations = k.session_keeper.escalations.saturating_add(1);
    emit_keeper(KeeperAction::Escalated, wallet, session, reward, caller);
    Ok(())
}

pub fn process_refund_escalated(ctx: Context<RefundEscalated>) -> Result<()> {
    let a = ctx.accounts;
    let session = a.refund.session.key();
    require_keys_eq!(
        a.keeper.session_keeper.session,
        session,
        RailsError::KeeperSessionMismatch
    );
    let caller = a.refund.caller.key();
    refund_buy(&mut a.refund)?;
    pay_escalation(
        &mut a.keeper,
        session,
        caller,
        &a.refund.token_program,
        &a.refund.usdc_mint,
    )
}

pub fn process_refund_ask_escalated(ctx: Context<RefundAskEscalated>) -> Result<()> {
    let a = ctx.accounts;
    let session = a.refund.session.key();
    require_keys_eq!(
        a.keeper.session_keeper.session,
        session,
        RailsError::KeeperSessionMismatch
    );
    let caller = a.refund.caller.key();
    refund_ask(&mut a.refund)?;
    pay_escalation(
        &mut a.keeper,
        session,
        caller,
        &a.refund.token_program,
        &a.refund.usdc_mint,
    )
}

pub fn process_release_assignment(ctx: Context<ReleaseAssignment>) -> Result<()> {
    let a = ctx.accounts;
    require!(a.session.open_escrows == 0, RailsError::SessionStillOpen);
    a.keeper.assignments = a.keeper.assignments.saturating_sub(1);
    emit_keeper(
        KeeperAction::Released,
        a.keeper.wallet,
        a.session.key(),
        0,
        a.caller.key(),
    );
    Ok(())
}

pub fn process_withdraw_keeper(ctx: Context<WithdrawKeeper>) -> Result<()> {
    let a = &ctx.accounts;
    require!(a.keeper.assignments == 0, RailsError::KeeperAssigned);
    let wallet = a.wallet.key();
    let seeds: &[&[u8]] = &[Keeper::SEED, wallet.as_ref(), &[a.keeper.bump]];
    let amount = a.vault.amount;
    token::transfer(
        &a.token_program,
        &a.usdc_mint,
        a.vault.to_account_info(),
        a.keeper_token.to_account_info(),
        a.keeper.to_account_info(),
        &[seeds],
        amount,
    )?;
    token::close(
        &a.token_program,
        a.vault.to_account_info(),
        a.wallet.to_account_info(),
        a.keeper.to_account_info(),
        &[seeds],
    )?;
    emit_keeper(
        KeeperAction::Withdrawn,
        wallet,
        Pubkey::default(),
        amount,
        wallet,
    );
    // `close = wallet` on the keeper account returns its rent.
    Ok(())
}

#[derive(Accounts)]
pub struct InitializeKeeperPolicy<'info> {
    #[account(seeds = [RailsConfig::SEED], bump = config.bump, has_one = authority)]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        init,
        payer = authority,
        space = 8 + KeeperPolicy::INIT_SPACE,
        seeds = [KeeperPolicy::SEED],
        bump
    )]
    pub policy: Box<Account<'info, KeeperPolicy>>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetKeeperPolicy<'info> {
    #[account(seeds = [RailsConfig::SEED], bump = config.bump, has_one = authority)]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(mut, seeds = [KeeperPolicy::SEED], bump = policy.bump)]
    pub policy: Box<Account<'info, KeeperPolicy>>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct RegisterKeeper<'info> {
    #[account(seeds = [RailsConfig::SEED], bump = config.bump)]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(seeds = [KeeperPolicy::SEED], bump = policy.bump)]
    pub policy: Box<Account<'info, KeeperPolicy>>,
    #[account(
        init,
        payer = wallet,
        space = 8 + Keeper::INIT_SPACE,
        seeds = [Keeper::SEED, wallet.key().as_ref()],
        bump
    )]
    pub keeper: Box<Account<'info, Keeper>>,
    #[account(
        init,
        payer = wallet,
        seeds = [Keeper::VAULT_SEED, keeper.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = keeper,
        token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = config.usdc_mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, constraint = keeper_token.mint == usdc_mint.key() @ RailsError::MintMismatch)]
    pub keeper_token: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub wallet: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct TopUpKeeper<'info> {
    #[account(seeds = [Keeper::SEED, wallet.key().as_ref()], bump = keeper.bump, has_one = wallet, has_one = vault)]
    pub keeper: Box<Account<'info, Keeper>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = vault.mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, constraint = keeper_token.mint == usdc_mint.key() @ RailsError::MintMismatch)]
    pub keeper_token: Box<InterfaceAccount<'info, TokenAccount>>,
    pub wallet: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct AssignKeeper<'info> {
    #[account(seeds = [KeeperPolicy::SEED], bump = policy.bump)]
    pub policy: Box<Account<'info, KeeperPolicy>>,
    #[account(
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump,
        has_one = operator
    )]
    pub session: Box<Account<'info, CashSession>>,
    #[account(mut, seeds = [Keeper::SEED, keeper.wallet.as_ref()], bump = keeper.bump, has_one = vault)]
    pub keeper: Box<Account<'info, Keeper>>,
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// One keeper per session; `init` fails on a second assignment.
    #[account(
        init,
        payer = operator,
        space = 8 + SessionKeeper::INIT_SPACE,
        seeds = [SessionKeeper::SEED, session.key().as_ref()],
        bump
    )]
    pub session_keeper: Box<Account<'info, SessionKeeper>>,
    #[account(mut)]
    pub operator: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// The keeper side of an escalated refund.
#[derive(Accounts)]
pub struct KeeperAccounts<'info> {
    #[account(
        mut,
        seeds = [SessionKeeper::SEED, session_keeper.session.as_ref()],
        bump = session_keeper.bump,
        has_one = keeper
    )]
    pub session_keeper: Box<Account<'info, SessionKeeper>>,
    #[account(mut, seeds = [Keeper::SEED, keeper.wallet.as_ref()], bump = keeper.bump, has_one = vault)]
    pub keeper: Box<Account<'info, Keeper>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// The caller's USDC account: where the reward goes. `dup`: the escrow
    /// owner may escalate its own late refund (it has the strongest reason
    /// to), and then this is the same account as the refund's
    /// `owner_token`. Safe: it is only a transfer destination here, its mint
    /// is checked, and Anchor never writes a token account back.
    #[account(mut, dup, constraint = caller_token.mint == vault.mint @ RailsError::MintMismatch)]
    pub caller_token: Box<InterfaceAccount<'info, TokenAccount>>,
}

#[derive(Accounts)]
pub struct RefundEscalated<'info> {
    pub refund: Refund<'info>,
    pub keeper: KeeperAccounts<'info>,
}

#[derive(Accounts)]
pub struct RefundAskEscalated<'info> {
    pub refund: RefundAsk<'info>,
    pub keeper: KeeperAccounts<'info>,
}

#[derive(Accounts)]
pub struct ReleaseAssignment<'info> {
    #[account(
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump
    )]
    pub session: Box<Account<'info, CashSession>>,
    #[account(
        mut,
        close = operator,
        seeds = [SessionKeeper::SEED, session.key().as_ref()],
        bump = session_keeper.bump,
        has_one = keeper
    )]
    pub session_keeper: Box<Account<'info, SessionKeeper>>,
    #[account(mut, seeds = [Keeper::SEED, keeper.wallet.as_ref()], bump = keeper.bump)]
    pub keeper: Box<Account<'info, Keeper>>,
    /// CHECK: rent refund to the session operator who paid it.
    #[account(mut, address = session.operator @ RailsError::Unauthorized)]
    pub operator: UncheckedAccount<'info>,
    /// Anyone.
    pub caller: Signer<'info>,
}

#[derive(Accounts)]
pub struct WithdrawKeeper<'info> {
    #[account(
        mut,
        close = wallet,
        seeds = [Keeper::SEED, wallet.key().as_ref()],
        bump = keeper.bump,
        has_one = wallet,
        has_one = vault
    )]
    pub keeper: Box<Account<'info, Keeper>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = vault.mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, constraint = keeper_token.mint == usdc_mint.key() @ RailsError::MintMismatch)]
    pub keeper_token: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub wallet: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}
