//! Escrow paths: bid/ask deposit (R1/R2), bond forfeit, permissionless
//! refunds for both sides.
//!
//! Invariants every path keeps (tested in `tests-svm`):
//! - Cash and asset only ever sit in per-escrow vaults owned by the escrow
//!   PDA; no engine- or operator-owned balances (AE-P10).
//! - Refunds go only to token accounts of the escrow's owner; bonds only
//!   to the session's snapshotted reserve.
//! - Refunds are never gated by `paused` / `depeg_breaker` (AE-P10, AC-11).
//! - Session totals track live escrows exactly (R4 reconciliation).

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::error::RailsError;
use crate::events::{EscrowAction, EscrowEvent};
use crate::instructions::token;
use crate::state::{BondRoute, CashSession, Escrow, RailsConfig, SessionStatus, Side, VaultDrain};

/// Commit-window checks shared by both deposit sides.
fn ensure_commit_open(config: &RailsConfig, session: &CashSession) -> Result<()> {
    config.ensure_accepting()?;
    let now = Clock::get()?.unix_timestamp;
    require!(
        session.status == SessionStatus::Open,
        RailsError::SessionNotOpen
    );
    require!(now < session.commit_end, RailsError::OutsideWindow);
    Ok(())
}

fn emit_escrow(
    action: EscrowAction,
    session: Pubkey,
    escrow: Pubkey,
    owner: Pubkey,
    amount: u64,
    caller: Pubkey,
) {
    emit!(EscrowEvent {
        action,
        session,
        escrow,
        owner,
        amount,
        unit: action.unit(),
        caller,
    });
}

/// Remove a closed escrow's holdings from the session totals, and book
/// what the refund found missing (#89). The recorded amounts leave the
/// live totals in full because the escrow is gone; the gap moves to the
/// session's shortfall counters, so the loss stays on the books.
fn untrack(
    session: &mut CashSession,
    cash: u64,
    bond: u64,
    asset: u64,
    cash_shortfall: u64,
    asset_shortfall: u64,
) -> Result<()> {
    let under = || error!(RailsError::MathUnderflow);
    let over = || error!(RailsError::MathOverflow);
    session.open_escrows = session.open_escrows.checked_sub(1).ok_or_else(under)?;
    session.total_cash = session.total_cash.checked_sub(cash).ok_or_else(under)?;
    session.total_bonds = session.total_bonds.checked_sub(bond).ok_or_else(under)?;
    session.total_asset = session.total_asset.checked_sub(asset).ok_or_else(under)?;
    session.total_cash_shortfall = session
        .total_cash_shortfall
        .checked_add(cash_shortfall)
        .ok_or_else(over)?;
    session.total_asset_shortfall = session
        .total_asset_shortfall
        .checked_add(asset_shortfall)
        .ok_or_else(over)?;
    Ok(())
}

/// AE-P3 buy side: lock `cash` (the bid's `qty × limit`, opaque here so the
/// bid stays sealed) plus the session bond into a fresh per-escrow vault.
pub fn process_deposit(ctx: Context<Deposit>, cash: u64) -> Result<()> {
    ensure_commit_open(&ctx.accounts.config, &ctx.accounts.session)?;
    let session = &ctx.accounts.session;
    require!(cash > 0, RailsError::ZeroAmount);
    require!(cash <= session.max_cash, RailsError::CashCapExceeded);
    let bond = session.bond_amount;
    let total = cash.checked_add(bond).ok_or(RailsError::MathOverflow)?;

    token::transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        ctx.accounts.owner_token.to_account_info(),
        ctx.accounts.vault.to_account_info(),
        ctx.accounts.owner.to_account_info(),
        &[],
        total,
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
        side: Side::Buy,
        cash,
        asset: 0,
        asset_vault: Pubkey::default(),
        bond,
        revealed: false,
        bump: ctx.bumps.escrow,
    });
    let session = &mut ctx.accounts.session;
    let over = || error!(RailsError::MathOverflow);
    session.open_escrows = session.open_escrows.checked_add(1).ok_or_else(over)?;
    session.total_cash = session.total_cash.checked_add(cash).ok_or_else(over)?;
    session.total_bonds = session.total_bonds.checked_add(bond).ok_or_else(over)?;

    emit_escrow(
        EscrowAction::Deposited,
        session_key,
        escrow_key,
        owner_key,
        cash,
        owner_key,
    );
    emit_escrow(
        EscrowAction::BondPosted,
        session_key,
        escrow_key,
        owner_key,
        bond,
        owner_key,
    );
    Ok(())
}

/// Sell side, step 1: create the seller's (empty) asset vault, authority =
/// the escrow PDA the following `deposit_ask` creates. Split out because a
/// vault of a `DefaultAccountState=Frozen` mint (NPCS) is frozen from birth:
/// the gate thaws it between this and `deposit_ask`. For an unfrozen mint
/// both go in one transaction. Owner-only, commit window only.
pub fn process_open_asset_vault(ctx: Context<OpenAssetVault>) -> Result<()> {
    ensure_commit_open(&ctx.accounts.config, &ctx.accounts.session)
}

/// AE-P3 sell side: lock `qty` of the session asset plus the USDC bond.
/// Seller escrow is what makes settlement a true DvP — a seller can only be
/// paid by delivering asset that is already locked here.
pub fn process_deposit_ask(ctx: Context<DepositAsk>, qty: u64) -> Result<()> {
    ensure_commit_open(&ctx.accounts.config, &ctx.accounts.session)?;
    require!(qty > 0, RailsError::ZeroAmount);
    require!(
        !ctx.accounts.asset_vault.is_frozen(),
        RailsError::AssetVaultFrozen
    );
    let bond = ctx.accounts.session.bond_amount;
    // The vault predates this ix, so it may hold donated dust; the escrow
    // tracks exactly `qty` and refund_ask returns everything held.
    let asset_before = ctx.accounts.asset_vault.amount;

    let owner_info = ctx.accounts.owner.to_account_info();
    token::transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        ctx.accounts.owner_usdc.to_account_info(),
        ctx.accounts.vault.to_account_info(),
        owner_info.clone(),
        &[],
        bond,
    )?;
    token::transfer(
        &ctx.accounts.asset_token_program,
        &ctx.accounts.asset_mint,
        ctx.accounts.owner_asset.to_account_info(),
        ctx.accounts.asset_vault.to_account_info(),
        owner_info,
        &[],
        qty,
    )?;
    ctx.accounts.vault.reload()?;
    ctx.accounts.asset_vault.reload()?;
    require!(
        ctx.accounts.vault.amount == bond
            && ctx.accounts.asset_vault.amount.checked_sub(asset_before) == Some(qty),
        RailsError::ConservationViolated
    );

    let session_key = ctx.accounts.session.key();
    let escrow_key = ctx.accounts.escrow.key();
    let owner_key = ctx.accounts.owner.key();
    ctx.accounts.escrow.set_inner(Escrow {
        session: session_key,
        owner: owner_key,
        vault: ctx.accounts.vault.key(),
        side: Side::Sell,
        cash: 0,
        asset: qty,
        asset_vault: ctx.accounts.asset_vault.key(),
        bond,
        revealed: false,
        bump: ctx.bumps.escrow,
    });
    let session = &mut ctx.accounts.session;
    let over = || error!(RailsError::MathOverflow);
    session.open_escrows = session.open_escrows.checked_add(1).ok_or_else(over)?;
    session.total_bonds = session.total_bonds.checked_add(bond).ok_or_else(over)?;
    session.total_asset = session.total_asset.checked_add(qty).ok_or_else(over)?;

    emit_escrow(
        EscrowAction::AssetDeposited,
        session_key,
        escrow_key,
        owner_key,
        qty,
        owner_key,
    );
    emit_escrow(
        EscrowAction::BondPosted,
        session_key,
        escrow_key,
        owner_key,
        bond,
        owner_key,
    );
    Ok(())
}

/// AE-P4: move an unrevealed escrow's bond to the reserve once reveals have
/// closed. Permissionless (crank); cash/asset stay until the refund. Works
/// for both sides — every escrow's bond sits in its USDC vault.
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
    token::transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        ctx.accounts.vault.to_account_info(),
        ctx.accounts.reserve.to_account_info(),
        ctx.accounts.escrow.to_account_info(),
        &[seeds],
        bond,
    )?;

    ctx.accounts.escrow.bond = 0;
    let session = &mut ctx.accounts.session;
    session.total_bonds = session
        .total_bonds
        .checked_sub(bond)
        .ok_or(RailsError::MathUnderflow)?;
    emit_escrow(
        EscrowAction::BondForfeited,
        session_key,
        ctx.accounts.escrow.key(),
        owner_key,
        bond,
        ctx.accounts.caller.key(),
    );
    Ok(())
}

/// Pay out an escrow's USDC vault per `route` and close it: the bond goes to
/// the reserve when forfeit; everything else (remaining cash, bond, donated
/// dust) goes to the owner. Returns `(to_reserve, to_owner, shortfall)`.
///
/// A vault holding less than `recorded` (a settlement mint with a permanent
/// delegate or clawback) still drains and closes. The forfeit is paid first,
/// up to what is held, and the owner gets the rest. The caller reports
/// `shortfall` as `CashShortfall` and books it on the session (#89). A
/// refund must never be blocked (AE-P10, doc 06 §11 H5, #62).
///
/// A frozen `owner_usdc` that would receive cash is rejected up front with
/// a typed error instead of failing inside the token program (#90); the
/// caller can retry with another thawed USDC account of the same owner.
#[allow(clippy::too_many_arguments)]
fn drain_usdc_vault<'info>(
    token_program: &Interface<'info, TokenInterface>,
    usdc_mint: &InterfaceAccount<'info, Mint>,
    vault: &InterfaceAccount<'info, TokenAccount>,
    escrow: AccountInfo<'info>,
    owner: AccountInfo<'info>,
    owner_usdc: &InterfaceAccount<'info, TokenAccount>,
    reserve: AccountInfo<'info>,
    seeds: &[&[u8]],
    recorded: u64,
    forfeit: u64,
) -> Result<(u64, u64, u64)> {
    // Anyone can send tokens to the vault; a donation must not be able to
    // block the close (griefing), so everything above the reserve's share
    // goes to the owner.
    let VaultDrain {
        to_reserve: forfeit,
        to_owner,
        shortfall,
    } = VaultDrain::compute(vault.amount, recorded, forfeit);
    require!(
        to_owner == 0 || !owner_usdc.is_frozen(),
        RailsError::RefundUsdcDestinationFrozen
    );
    let vault_info = vault.to_account_info();
    token::transfer(
        token_program,
        usdc_mint,
        vault_info.clone(),
        reserve,
        escrow.clone(),
        &[seeds],
        forfeit,
    )?;
    token::transfer(
        token_program,
        usdc_mint,
        vault_info.clone(),
        owner_usdc.to_account_info(),
        escrow.clone(),
        &[seeds],
        to_owner,
    )?;
    token::close(token_program, vault_info, owner, escrow, &[seeds])?;
    Ok((forfeit, to_owner, shortfall))
}

/// AE-P10 / AC-11: return a **buy** escrow to its owner. Permissionless —
/// any signer may call it once `CashSession::refund_route` allows, including
/// while paused or with the depeg breaker tripped, so a dead crank or a
/// halted venue can never trap cash. After settlement this is also how the
/// buyer's unfilled excess comes back (AE-P8). Forfeits the bond on the way
/// out when AE-P4 says so, then closes vault + escrow, rent to the owner.
pub fn process_refund(ctx: Context<Refund>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let escrow = &ctx.accounts.escrow;
    // A sell escrow also has an asset vault; closing it here would orphan
    // the asset. Sellers use `refund_ask`.
    require!(escrow.side == Side::Buy, RailsError::WrongSide);
    let route = ctx.accounts.session.refund_route(escrow.revealed, now)?;
    let (cash, bond) = (escrow.cash, escrow.bond);
    let forfeit = if route == BondRoute::ToReserve {
        bond
    } else {
        0
    };

    let session_key = ctx.accounts.session.key();
    let owner_key = escrow.owner;
    let escrow_key = escrow.key();
    let seeds: &[&[u8]] = &[
        Escrow::SEED,
        session_key.as_ref(),
        owner_key.as_ref(),
        &[escrow.bump],
    ];
    let recorded = cash.checked_add(bond).ok_or(RailsError::MathOverflow)?;
    let (to_reserve, to_owner, cash_shortfall) = drain_usdc_vault(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        &ctx.accounts.vault,
        ctx.accounts.escrow.to_account_info(),
        ctx.accounts.owner.to_account_info(),
        &ctx.accounts.owner_token,
        ctx.accounts.reserve.to_account_info(),
        seeds,
        recorded,
        forfeit,
    )?;
    untrack(&mut ctx.accounts.session, cash, bond, 0, cash_shortfall, 0)?;

    let caller = ctx.accounts.caller.key();
    if to_reserve > 0 {
        emit_escrow(
            EscrowAction::BondForfeited,
            session_key,
            escrow_key,
            owner_key,
            to_reserve,
            caller,
        );
    }
    emit_escrow(
        EscrowAction::Refunded,
        session_key,
        escrow_key,
        owner_key,
        to_owner,
        caller,
    );
    if cash_shortfall > 0 {
        emit_escrow(
            EscrowAction::CashShortfall,
            session_key,
            escrow_key,
            owner_key,
            cash_shortfall,
            caller,
        );
    }
    // `close = owner` on the escrow account returns its rent.
    Ok(())
}

/// AE-P10 sell side: return unsold asset + bond (or just the asset, when
/// the bond is forfeit) to the seller. Same permissionless, never-pausable
/// rules as `refund`; closes both vaults and the escrow. Completes even if
/// the asset vault was drained below the recorded amount (#62).
///
/// The asset can only go to a **thawed** account of the seller (#90). NPCS
/// re-freezes a holder whose attestation lapses, so the seller's usual
/// account may be frozen: the call then fails with
/// `RefundAssetDestinationFrozen` before anything moves, and succeeds when
/// re-sent with any other thawed NPCS account of the same owner (or after
/// the issuer thaws this one). With a vault drained to zero, no asset moves
/// and a frozen destination does not block the bond.
pub fn process_refund_ask(ctx: Context<RefundAsk>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let escrow = &ctx.accounts.escrow;
    require!(escrow.side == Side::Sell, RailsError::WrongSide);
    let route = ctx.accounts.session.refund_route(escrow.revealed, now)?;
    let (asset, bond) = (escrow.asset, escrow.bond);
    let forfeit = if route == BondRoute::ToReserve {
        bond
    } else {
        0
    };

    let session_key = ctx.accounts.session.key();
    let owner_key = escrow.owner;
    let escrow_key = escrow.key();
    let seeds: &[&[u8]] = &[
        Escrow::SEED,
        session_key.as_ref(),
        owner_key.as_ref(),
        &[escrow.bump],
    ];
    // Asset: everything held (incl. any donated dust) back to the seller.
    // The vault can hold *less* than recorded: an asset with a permanent
    // delegate (NPCS, SRS 08 §3.1) can pull from it at any time. That must
    // not block the refund, or the seller's bond is stranded forever
    // (AE-P10 / doc 06 §11 H5, #62). Return what is there and record the
    // gap as `AssetShortfall`. Checked before any cash moves (#90).
    let held = ctx.accounts.asset_vault.amount;
    let shortfall = asset.saturating_sub(held);
    require!(
        held == 0 || !ctx.accounts.owner_asset.is_frozen(),
        RailsError::RefundAssetDestinationFrozen
    );

    let escrow_info = ctx.accounts.escrow.to_account_info();
    let owner_info = ctx.accounts.owner.to_account_info();
    let (to_reserve, to_owner, cash_shortfall) = drain_usdc_vault(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        &ctx.accounts.vault,
        escrow_info.clone(),
        owner_info.clone(),
        &ctx.accounts.owner_usdc,
        ctx.accounts.reserve.to_account_info(),
        seeds,
        bond,
        forfeit,
    )?;

    let asset_vault_info = ctx.accounts.asset_vault.to_account_info();
    if held > 0 {
        token::transfer(
            &ctx.accounts.asset_token_program,
            &ctx.accounts.asset_mint,
            asset_vault_info.clone(),
            ctx.accounts.owner_asset.to_account_info(),
            escrow_info.clone(),
            &[seeds],
            held,
        )?;
    }
    token::close(
        &ctx.accounts.asset_token_program,
        asset_vault_info,
        owner_info,
        escrow_info,
        &[seeds],
    )?;
    untrack(
        &mut ctx.accounts.session,
        0,
        bond,
        asset,
        cash_shortfall,
        shortfall,
    )?;

    let caller = ctx.accounts.caller.key();
    if to_reserve > 0 {
        emit_escrow(
            EscrowAction::BondForfeited,
            session_key,
            escrow_key,
            owner_key,
            to_reserve,
            caller,
        );
    }
    emit_escrow(
        EscrowAction::Refunded,
        session_key,
        escrow_key,
        owner_key,
        to_owner,
        caller,
    );
    if cash_shortfall > 0 {
        emit_escrow(
            EscrowAction::CashShortfall,
            session_key,
            escrow_key,
            owner_key,
            cash_shortfall,
            caller,
        );
    }
    emit_escrow(
        EscrowAction::AssetReturned,
        session_key,
        escrow_key,
        owner_key,
        held,
        caller,
    );
    if shortfall > 0 {
        emit_escrow(
            EscrowAction::AssetShortfall,
            session_key,
            escrow_key,
            owner_key,
            shortfall,
            caller,
        );
    }
    Ok(())
}

// Token, mint and session accounts are boxed (heap) throughout: unboxed,
// `Refund::try_accounts` needs a 4.5 KiB frame and overflows the 4 KiB SBF
// stack limit (`anchor build` warns; runtime behaviour is UB).
#[derive(Accounts)]
pub struct Deposit<'info> {
    /// Halt switches (`ensure_accepting`).
    #[account(seeds = [RailsConfig::SEED], bump = config.bump)]
    pub config: Box<Account<'info, RailsConfig>>,
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
    pub escrow: Box<Account<'info, Escrow>>,
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
pub struct OpenAssetVault<'info> {
    /// Halt switches (`ensure_accepting`).
    #[account(seeds = [RailsConfig::SEED], bump = config.bump)]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump
    )]
    pub session: Box<Account<'info, CashSession>>,
    /// The seller's future escrow PDA — only its address is used here.
    /// CHECK: seeds-derived; `deposit_ask` creates it.
    #[account(seeds = [Escrow::SEED, session.key().as_ref(), owner.key().as_ref()], bump)]
    pub escrow: UncheckedAccount<'info>,
    #[account(
        init,
        payer = owner,
        seeds = [Escrow::ASSET_VAULT_SEED, escrow.key().as_ref()],
        bump,
        token::mint = asset_mint,
        token::authority = escrow,
        token::token_program = asset_token_program,
    )]
    pub asset_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = session.asset_mint @ RailsError::AssetMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub owner: Signer<'info>,
    /// Token program of the asset mint (Token-2022 for NPCS).
    pub asset_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositAsk<'info> {
    /// Halt switches (`ensure_accepting`).
    #[account(seeds = [RailsConfig::SEED], bump = config.bump)]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        mut,
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump
    )]
    pub session: Box<Account<'info, CashSession>>,
    /// Same seed as a bid escrow: one escrow per (session, owner), so a
    /// participant cannot be both buyer and seller in one session.
    #[account(
        init,
        payer = owner,
        space = 8 + Escrow::INIT_SPACE,
        seeds = [Escrow::SEED, session.key().as_ref(), owner.key().as_ref()],
        bump
    )]
    pub escrow: Box<Account<'info, Escrow>>,
    /// USDC vault (bond only), authority = escrow PDA.
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
    /// Asset vault from `open_asset_vault`, authority = escrow PDA.
    #[account(
        mut,
        seeds = [Escrow::ASSET_VAULT_SEED, escrow.key().as_ref()],
        bump,
        token::mint = asset_mint,
        token::authority = escrow,
        token::token_program = asset_token_program,
    )]
    pub asset_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = session.usdc_mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = session.asset_mint @ RailsError::AssetMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    /// Seller's USDC (bond source).
    #[account(
        mut,
        constraint = owner_usdc.mint == usdc_mint.key() @ RailsError::MintMismatch,
    )]
    pub owner_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Seller's asset.
    #[account(
        mut,
        constraint = owner_asset.mint == asset_mint.key() @ RailsError::AssetMismatch,
    )]
    pub owner_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub owner: Signer<'info>,
    /// Token program of the settlement mint.
    pub token_program: Interface<'info, TokenInterface>,
    /// Token program of the asset mint (Token-2022 for NPCS).
    pub asset_token_program: Interface<'info, TokenInterface>,
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
    pub escrow: Box<Account<'info, Escrow>>,
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
    pub escrow: Box<Account<'info, Escrow>>,
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

#[derive(Accounts)]
pub struct RefundAsk<'info> {
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
        has_one = asset_vault,
        close = owner
    )]
    pub escrow: Box<Account<'info, Escrow>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub asset_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: bound to `escrow.owner` by `has_one`; receives rent only.
    #[account(mut)]
    pub owner: UncheckedAccount<'info>,
    /// Bond destination: the seller's own USDC account.
    #[account(
        mut,
        constraint = owner_usdc.owner == escrow.owner @ RailsError::NotEscrowOwner,
        constraint = owner_usdc.mint == usdc_mint.key() @ RailsError::MintMismatch,
    )]
    pub owner_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Asset destination: the seller's own asset account.
    #[account(
        mut,
        constraint = owner_asset.owner == escrow.owner @ RailsError::NotEscrowOwner,
        constraint = owner_asset.mint == asset_mint.key() @ RailsError::AssetMismatch,
    )]
    pub owner_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, address = session.reserve @ RailsError::ReserveMismatch)]
    pub reserve: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = session.usdc_mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = session.asset_mint @ RailsError::AssetMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
}
