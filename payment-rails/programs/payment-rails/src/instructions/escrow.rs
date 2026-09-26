//! Escrow cash paths (R1): deposit, bond forfeit, permissionless refund.
//!
//! Invariants every path keeps (tested in `tests-svm`):
//! - Cash only ever sits in a per-escrow vault owned by the escrow PDA; no
//!   engine- or operator-owned balances (AE-P10).
//! - Refunds go only to a token account of the escrow's owner; bonds only
//!   to the session's snapshotted reserve.
//! - Refunds are never gated by `paused` / `depeg_breaker` (AE-P10, AC-11).
//! - Session totals track live escrows exactly (R4 reconciliation).

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    close_account, transfer_checked, CloseAccount, Mint, TokenAccount, TokenInterface,
    TransferChecked,
};

use crate::error::RailsError;
use crate::events::{EscrowAction, EscrowEvent};
use crate::state::{BondRoute, CashSession, Escrow, RailsConfig, SessionStatus};

/// AE-P3: lock `cash` (the bid's `qty × limit`, opaque here so the bid
/// stays sealed) plus the session bond into a fresh per-escrow vault.
pub fn process_deposit(ctx: Context<Deposit>, cash: u64) -> Result<()> {
    ctx.accounts.config.ensure_accepting()?;
    let now = Clock::get()?.unix_timestamp;
    let session = &ctx.accounts.session;
    require!(
        session.status == SessionStatus::Open,
        RailsError::SessionNotOpen
    );
    require!(now < session.commit_end, RailsError::OutsideWindow);
    require!(cash > 0, RailsError::ZeroAmount);
    require!(cash <= session.max_cash, RailsError::CashCapExceeded);
    let bond = session.bond_amount;
    let total = cash.checked_add(bond).ok_or(RailsError::MathOverflow)?;

    transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.owner_token.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
                authority: ctx.accounts.owner.to_account_info(),
            },
        ),
        total,
        ctx.accounts.usdc_mint.decimals,
    )?;
    // A transfer-fee or otherwise lossy settlement mint would under-fund the
    // vault; refuse rather than escrow less than we record (conservation).
    ctx.accounts.vault.reload()?;
    require!(
        ctx.accounts.vault.amount == total,
        RailsError::ConservationViolated
    );

    let session_key = ctx.accounts.session.key();
    let escrow_key = ctx.accounts.escrow.key();
    let owner_key = ctx.accounts.owner.key();
    ctx.accounts.escrow.set_inner(Escrow {
        session: session_key,
        owner: owner_key,
        vault: ctx.accounts.vault.key(),
        cash,
        bond,
        revealed: false,
        bump: ctx.bumps.escrow,
    });
    let session = &mut ctx.accounts.session;
    session.open_escrows = session
        .open_escrows
        .checked_add(1)
        .ok_or(RailsError::MathOverflow)?;
    session.total_cash = session
        .total_cash
        .checked_add(cash)
        .ok_or(RailsError::MathOverflow)?;
    session.total_bonds = session
        .total_bonds
        .checked_add(bond)
        .ok_or(RailsError::MathOverflow)?;

    for (action, amount) in [
        (EscrowAction::Deposited, cash),
        (EscrowAction::BondPosted, bond),
    ] {
        emit!(EscrowEvent {
            action,
            session: session_key,
            escrow: escrow_key,
            owner: owner_key,
            amount,
            caller: owner_key,
        });
    }
    Ok(())
}

/// AE-P4: move an unrevealed escrow's bond to the reserve once reveals have
/// closed. Permissionless (crank); the cash stays until `refund`.
pub fn process_forfeit_bond(ctx: Context<ForfeitBond>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let bond = ctx.accounts.escrow.bond;
    require!(
        bond > 0
            && ctx
                .accounts
                .session
                .bond_forfeit(ctx.accounts.escrow.revealed, now),
        RailsError::BondNotForfeitable
    );

    let session_key = ctx.accounts.session.key();
    let owner_key = ctx.accounts.escrow.owner;
    let seeds: &[&[u8]] = &[
        Escrow::SEED,
        session_key.as_ref(),
        owner_key.as_ref(),
        &[ctx.accounts.escrow.bump],
    ];
    vault_transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.vault,
        &ctx.accounts.usdc_mint,
        ctx.accounts.reserve.to_account_info(),
        ctx.accounts.escrow.to_account_info(),
        seeds,
        bond,
    )?;

    ctx.accounts.escrow.bond = 0;
    let session = &mut ctx.accounts.session;
    session.total_bonds = session
        .total_bonds
        .checked_sub(bond)
        .ok_or(RailsError::MathUnderflow)?;
    emit!(EscrowEvent {
        action: EscrowAction::BondForfeited,
        session: session_key,
        escrow: ctx.accounts.escrow.key(),
        owner: owner_key,
        amount: bond,
        caller: ctx.accounts.caller.key(),
    });
    Ok(())
}

/// AE-P10 / AC-11: return an escrow to its owner. **Permissionless** — any
/// signer may call it once `CashSession::refund_route` allows, including
/// while paused or with the depeg breaker tripped, so a dead crank or a
/// halted venue can never trap cash. Forfeits the bond on the way out when
/// AE-P4 says so, then closes vault + escrow with rent back to the owner.
pub fn process_refund(ctx: Context<Refund>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let escrow = &ctx.accounts.escrow;
    let route = ctx.accounts.session.refund_route(escrow.revealed, now)?;
    let (cash, bond) = (escrow.cash, escrow.bond);
    let to_reserve = match route {
        BondRoute::ToReserve => bond,
        BondRoute::ToOwner => 0,
    };
    // Anyone can send tokens to the vault; a donation must not be able to
    // block the close (griefing), so everything above the reserve's share
    // goes to the owner.
    let held = ctx.accounts.vault.amount;
    let recorded = cash.checked_add(bond).ok_or(RailsError::MathOverflow)?;
    require!(held >= recorded, RailsError::ConservationViolated);
    let to_owner = held - to_reserve;

    let session_key = ctx.accounts.session.key();
    let owner_key = escrow.owner;
    let escrow_key = escrow.key();
    let seeds: &[&[u8]] = &[
        Escrow::SEED,
        session_key.as_ref(),
        owner_key.as_ref(),
        &[escrow.bump],
    ];
    let escrow_info = ctx.accounts.escrow.to_account_info();
    if to_reserve > 0 {
        vault_transfer(
            &ctx.accounts.token_program,
            &ctx.accounts.vault,
            &ctx.accounts.usdc_mint,
            ctx.accounts.reserve.to_account_info(),
            escrow_info.clone(),
            seeds,
            to_reserve,
        )?;
    }
    if to_owner > 0 {
        vault_transfer(
            &ctx.accounts.token_program,
            &ctx.accounts.vault,
            &ctx.accounts.usdc_mint,
            ctx.accounts.owner_token.to_account_info(),
            escrow_info.clone(),
            seeds,
            to_owner,
        )?;
    }
    close_account(CpiContext::new_with_signer(
        ctx.accounts.token_program.key(),
        CloseAccount {
            account: ctx.accounts.vault.to_account_info(),
            destination: ctx.accounts.owner.to_account_info(),
            authority: escrow_info,
        },
        &[seeds],
    ))?;

    let session = &mut ctx.accounts.session;
    session.open_escrows = session
        .open_escrows
        .checked_sub(1)
        .ok_or(RailsError::MathUnderflow)?;
    session.total_cash = session
        .total_cash
        .checked_sub(cash)
        .ok_or(RailsError::MathUnderflow)?;
    session.total_bonds = session
        .total_bonds
        .checked_sub(bond)
        .ok_or(RailsError::MathUnderflow)?;

    let caller = ctx.accounts.caller.key();
    if to_reserve > 0 {
        emit!(EscrowEvent {
            action: EscrowAction::BondForfeited,
            session: session_key,
            escrow: escrow_key,
            owner: owner_key,
            amount: to_reserve,
            caller,
        });
    }
    emit!(EscrowEvent {
        action: EscrowAction::Refunded,
        session: session_key,
        escrow: escrow_key,
        owner: owner_key,
        amount: to_owner,
        caller,
    });
    // `close = owner` on the escrow account returns its rent.
    Ok(())
}

/// Vault → `to`, signed by the escrow PDA.
fn vault_transfer<'info>(
    token_program: &Interface<'info, TokenInterface>,
    vault: &InterfaceAccount<'info, TokenAccount>,
    mint: &InterfaceAccount<'info, Mint>,
    to: AccountInfo<'info>,
    escrow: AccountInfo<'info>,
    seeds: &[&[u8]],
    amount: u64,
) -> Result<()> {
    transfer_checked(
        CpiContext::new_with_signer(
            token_program.key(),
            TransferChecked {
                from: vault.to_account_info(),
                mint: mint.to_account_info(),
                to,
                authority: escrow,
            },
            &[seeds],
        ),
        amount,
        mint.decimals,
    )
}

// Token, mint and session accounts are boxed (heap) in all three structs:
// unboxed, `Refund::try_accounts` needs a 4.5 KiB frame and overflows the
// 4 KiB SBF stack limit (`anchor build` warns; runtime behaviour is UB).
#[derive(Accounts)]
pub struct Deposit<'info> {
    /// Halt switches (`ensure_accepting`).
    #[account(seeds = [RailsConfig::SEED], bump = config.bump)]
    pub config: Account<'info, RailsConfig>,
    #[account(
        mut,
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump
    )]
    pub session: Box<Account<'info, CashSession>>,
    /// One escrow per (session, owner); the owner must sign, so nobody can
    /// squat another investor's escrow address.
    #[account(
        init,
        payer = owner,
        space = 8 + Escrow::INIT_SPACE,
        seeds = [Escrow::SEED, session.key().as_ref(), owner.key().as_ref()],
        bump
    )]
    pub escrow: Account<'info, Escrow>,
    /// Dedicated vault, authority = escrow PDA.
    #[account(
        init,
        payer = owner,
        seeds = [Escrow::VAULT_SEED, escrow.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = escrow,
        token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = session.usdc_mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    /// Depositor's USDC (token program enforces `owner` authority).
    #[account(
        mut,
        constraint = owner_token.mint == usdc_mint.key() @ RailsError::MintMismatch,
    )]
    pub owner_token: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub owner: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ForfeitBond<'info> {
    /// Anyone (crank).
    pub caller: Signer<'info>,
    #[account(
        mut,
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump
    )]
    pub session: Box<Account<'info, CashSession>>,
    #[account(
        mut,
        seeds = [Escrow::SEED, session.key().as_ref(), escrow.owner.as_ref()],
        bump = escrow.bump,
        has_one = session,
        has_one = vault
    )]
    pub escrow: Account<'info, Escrow>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Session's snapshotted reserve — the only bond destination.
    #[account(mut, address = session.reserve @ RailsError::ReserveMismatch)]
    pub reserve: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = session.usdc_mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Refund<'info> {
    /// Anyone — refunds are permissionless (AE-P10). Pays only the tx fee.
    pub caller: Signer<'info>,
    #[account(
        mut,
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump
    )]
    pub session: Box<Account<'info, CashSession>>,
    #[account(
        mut,
        seeds = [Escrow::SEED, session.key().as_ref(), escrow.owner.as_ref()],
        bump = escrow.bump,
        has_one = session,
        has_one = owner,
        has_one = vault,
        close = owner
    )]
    pub escrow: Account<'info, Escrow>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: bound to `escrow.owner` by `has_one`; receives rent only.
    #[account(mut)]
    pub owner: UncheckedAccount<'info>,
    /// Refund destination: must belong to the escrow owner — a third-party
    /// caller can trigger the refund but can never redirect it.
    #[account(
        mut,
        constraint = owner_token.owner == escrow.owner @ RailsError::NotEscrowOwner,
        constraint = owner_token.mint == usdc_mint.key() @ RailsError::MintMismatch,
    )]
    pub owner_token: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Session's snapshotted reserve (receives a forfeited bond, if any).
    #[account(mut, address = session.reserve @ RailsError::ReserveMismatch)]
    pub reserve: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = session.usdc_mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
}
