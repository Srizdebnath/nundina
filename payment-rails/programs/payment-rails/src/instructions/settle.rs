//! DvP settlement (R2; AE-P7/P8): one fill per instruction, loopable by the
//! crank, idempotent via a fill-receipt PDA.
//!
//! Each fill is atomic delivery-versus-payment: the asset leg (seller's
//! escrowed asset → buyer) and the cash leg (buyer's escrowed cash → seller,
//! fee receiver, reserve) happen in one instruction or not at all. So the
//! operator only chooses the *matching* (it is trusted to apply the
//! auction's clearing), but cannot move value to anyone outside the
//! session's participants and the protocol's snapshotted fee/reserve
//! accounts: every destination is pinned by owner or by snapshot.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::error::RailsError;
use crate::events::{ExemptionPath, FillSettled, GateCheck, TransferInstruction};
use crate::instructions::token;
use crate::state::{CashSession, Escrow, FillReceipt, FillSplit, RailsConfig, SessionStatus, Side};

/// Preconditions shared by both fill paths. Returns `now`.
fn ensure_settleable(config: &RailsConfig, session: &CashSession) -> Result<i64> {
    // Depeg / pause halt settlement (06 §2 F4/M5) — refunds stay open.
    config.ensure_accepting()?;
    let now = Clock::get()?.unix_timestamp;
    require!(
        session.status == SessionStatus::Open,
        RailsError::SessionNotOpen
    );
    require!(
        now >= session.reveal_end && now < session.settle_deadline,
        RailsError::OutsideWindow
    );
    Ok(now)
}

/// Asset leg: seller escrow's asset vault → `to`, signed by the escrow PDA.
fn deliver_asset<'info>(
    session_key: &Pubkey,
    seller: &Account<'info, Escrow>,
    asset_vault: &InterfaceAccount<'info, TokenAccount>,
    asset_mint: &InterfaceAccount<'info, Mint>,
    asset_token_program: &Interface<'info, TokenInterface>,
    to: AccountInfo<'info>,
    qty: u64,
) -> Result<()> {
    let seeds: &[&[u8]] = &[
        Escrow::SEED,
        session_key.as_ref(),
        seller.owner.as_ref(),
        &[seller.bump],
    ];
    token::transfer(
        asset_token_program,
        asset_mint,
        asset_vault.to_account_info(),
        to,
        seller.to_account_info(),
        &[seeds],
        qty,
    )
}

/// Session-level bookkeeping + receipt + event for a settled fill.
#[allow(clippy::too_many_arguments)]
fn record_fill(
    session_key: Pubkey,
    session: &mut CashSession,
    receipt: &mut Account<FillReceipt>,
    receipt_bump: u8,
    fill_index: u32,
    buyer: Pubkey,
    seller: Pubkey,
    qty: u64,
    price: u64,
    split: FillSplit,
    rofr: bool,
    now: i64,
    disclosure_hash: [u8; 32],
) -> Result<()> {
    let over = || error!(RailsError::MathOverflow);
    session.fills_settled = session.fills_settled.checked_add(1).ok_or_else(over)?;
    session.volume_settled = session.volume_settled.checked_add(qty).ok_or_else(over)?;
    session.gross_settled = session
        .gross_settled
        .checked_add(split.gross)
        .ok_or_else(over)?;
    session.total_asset = session
        .total_asset
        .checked_sub(qty)
        .ok_or(RailsError::MathUnderflow)?;
    receipt.set_inner(FillReceipt {
        session: session_key,
        fill_index,
        buyer,
        seller,
        qty,
        price,
        split,
        rofr,
        settled_at: now,
        bump: receipt_bump,
    });
    emit!(FillSettled {
        session: session_key,
        fill_index,
        buyer,
        seller,
        qty,
        price,
        gross: split.gross,
        to_seller: split.to_seller,
        fee: split.fee,
        to_reserve: split.to_reserve,
        rofr,
        disclosure_hash,
    });
    // AE-P8 transfer record. No lot ledger and no gate re-check exist yet,
    // so this states that honestly instead of omitting the fields (#61, #53).
    emit!(TransferInstruction {
        session: session_key,
        fill_index,
        from: seller,
        to: buyer,
        qty,
        price,
        lot_ids: Vec::new(),
        exemption_path: ExemptionPath::NotRecorded,
        gate_check: GateCheck::NotPerformed,
        disclosure_hash,
    });
    Ok(())
}

/// AE-P8: settle one fill between a revealed buyer escrow and a revealed
/// seller escrow at `price` (micro-USDC per whole asset unit).
///
/// Buyer pays `gross = ceil(qty × price / 10^dec)` from escrow; seller
/// receives `gross − fee − reserve_cut`; `qty` asset moves seller → buyer.
/// The buyer's unfilled excess and bond come back via `refund` once the
/// operator calls `finish_session` (or at the hard deadline).
pub fn process_settle_fill(
    ctx: Context<SettleFill>,
    fill_index: u32,
    qty: u64,
    price: u64,
    disclosure_hash: [u8; 32],
) -> Result<()> {
    let now = ensure_settleable(&ctx.accounts.config, &ctx.accounts.session)?;
    let buyer = &ctx.accounts.buyer_escrow;
    let seller = &ctx.accounts.seller_escrow;
    require!(
        buyer.side == Side::Buy && seller.side == Side::Sell,
        RailsError::WrongSide
    );
    require!(buyer.revealed && seller.revealed, RailsError::NotRevealed);
    let session = &ctx.accounts.session;
    let split = FillSplit::compute(
        qty,
        price,
        session.asset_decimals,
        session.fee_bps,
        session.reserve_bps,
    )?;
    require!(
        split.gross <= buyer.cash && qty <= seller.asset,
        RailsError::InsufficientEscrow
    );

    // Cash leg: buyer vault → seller / fee / reserve.
    let session_key = session.key();
    let buyer_seeds: &[&[u8]] = &[
        Escrow::SEED,
        session_key.as_ref(),
        buyer.owner.as_ref(),
        &[buyer.bump],
    ];
    let buyer_info = buyer.to_account_info();
    let buyer_vault = ctx.accounts.buyer_vault.to_account_info();
    for (to, amount) in [
        (ctx.accounts.seller_usdc.to_account_info(), split.to_seller),
        (ctx.accounts.fee_receiver.to_account_info(), split.fee),
        (ctx.accounts.reserve.to_account_info(), split.to_reserve),
    ] {
        token::transfer(
            &ctx.accounts.token_program,
            &ctx.accounts.usdc_mint,
            buyer_vault.clone(),
            to,
            buyer_info.clone(),
            &[buyer_seeds],
            amount,
        )?;
    }
    // Asset leg: seller vault → buyer.
    deliver_asset(
        &session_key,
        seller,
        &ctx.accounts.seller_asset_vault,
        &ctx.accounts.asset_mint,
        &ctx.accounts.asset_token_program,
        ctx.accounts.buyer_asset.to_account_info(),
        qty,
    )?;

    let (buyer_owner, seller_owner) = (buyer.owner, seller.owner);
    ctx.accounts.buyer_escrow.cash -= split.gross; // ≤ cash, checked above
    ctx.accounts.seller_escrow.asset -= qty; // ≤ asset, checked above
    let session = &mut ctx.accounts.session;
    session.total_cash = session
        .total_cash
        .checked_sub(split.gross)
        .ok_or(RailsError::MathUnderflow)?;
    record_fill(
        session_key,
        session,
        &mut ctx.accounts.receipt,
        ctx.bumps.receipt,
        fill_index,
        buyer_owner,
        seller_owner,
        qty,
        price,
        split,
        false,
        now,
        disclosure_hash,
    )
}

/// AE-P7: the issuer exercises its ROFR on a seller's escrowed asset at the
/// clearing price. Cash comes straight from the issuer's own USDC account
/// (issuer signs); the asset goes to the issuer's own asset account. Both
/// the operator (fill bookkeeping) and the issuer must sign.
pub fn process_settle_rofr_fill(
    ctx: Context<SettleRofrFill>,
    fill_index: u32,
    qty: u64,
    price: u64,
    disclosure_hash: [u8; 32],
) -> Result<()> {
    let now = ensure_settleable(&ctx.accounts.config, &ctx.accounts.session)?;
    let session = &ctx.accounts.session;
    require!(
        session.rofr_authority != Pubkey::default()
            && ctx.accounts.issuer.key() == session.rofr_authority,
        RailsError::NotRofrAuthority
    );
    let seller = &ctx.accounts.seller_escrow;
    require!(seller.side == Side::Sell, RailsError::WrongSide);
    require!(seller.revealed, RailsError::NotRevealed);
    let split = FillSplit::compute(
        qty,
        price,
        session.asset_decimals,
        session.fee_bps,
        session.reserve_bps,
    )?;
    require!(qty <= seller.asset, RailsError::InsufficientEscrow);

    // Cash leg: issuer's own account → seller / fee / reserve.
    let issuer_info = ctx.accounts.issuer.to_account_info();
    let issuer_usdc = ctx.accounts.issuer_usdc.to_account_info();
    for (to, amount) in [
        (ctx.accounts.seller_usdc.to_account_info(), split.to_seller),
        (ctx.accounts.fee_receiver.to_account_info(), split.fee),
        (ctx.accounts.reserve.to_account_info(), split.to_reserve),
    ] {
        token::transfer(
            &ctx.accounts.token_program,
            &ctx.accounts.usdc_mint,
            issuer_usdc.clone(),
            to,
            issuer_info.clone(),
            &[],
            amount,
        )?;
    }
    let session_key = session.key();
    deliver_asset(
        &session_key,
        seller,
        &ctx.accounts.seller_asset_vault,
        &ctx.accounts.asset_mint,
        &ctx.accounts.asset_token_program,
        ctx.accounts.issuer_asset.to_account_info(),
        qty,
    )?;

    let seller_owner = seller.owner;
    ctx.accounts.seller_escrow.asset -= qty; // ≤ asset, checked above
    record_fill(
        session_key,
        &mut ctx.accounts.session,
        &mut ctx.accounts.receipt,
        ctx.bumps.receipt,
        fill_index,
        ctx.accounts.issuer.key(),
        seller_owner,
        qty,
        price,
        split,
        true,
        now,
        disclosure_hash,
    )
}

#[derive(Accounts)]
#[instruction(fill_index: u32)]
pub struct SettleFill<'info> {
    /// Halt switches (depeg breaker blocks settlement).
    #[account(seeds = [RailsConfig::SEED], bump = config.bump)]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        mut,
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump,
        has_one = operator @ RailsError::NotOperator
    )]
    pub session: Box<Account<'info, CashSession>>,
    /// Operator (crank-driven engine signer); pays the receipt rent.
    #[account(mut)]
    pub operator: Signer<'info>,
    /// Idempotency: created here, so a replayed `fill_index` fails.
    #[account(
        init,
        payer = operator,
        space = 8 + FillReceipt::INIT_SPACE,
        seeds = [FillReceipt::SEED, session.key().as_ref(), &fill_index.to_le_bytes()],
        bump
    )]
    pub receipt: Box<Account<'info, FillReceipt>>,
    #[account(
        mut,
        seeds = [Escrow::SEED, session.key().as_ref(), buyer_escrow.owner.as_ref()],
        bump = buyer_escrow.bump,
        has_one = session
    )]
    pub buyer_escrow: Box<Account<'info, Escrow>>,
    #[account(mut, address = buyer_escrow.vault)]
    pub buyer_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        seeds = [Escrow::SEED, session.key().as_ref(), seller_escrow.owner.as_ref()],
        bump = seller_escrow.bump,
        has_one = session
    )]
    pub seller_escrow: Box<Account<'info, Escrow>>,
    #[account(mut, address = seller_escrow.asset_vault)]
    pub seller_asset_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Cash destination: must belong to the seller.
    #[account(
        mut,
        constraint = seller_usdc.owner == seller_escrow.owner @ RailsError::NotEscrowOwner,
        constraint = seller_usdc.mint == usdc_mint.key() @ RailsError::MintMismatch,
    )]
    pub seller_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Asset destination: must belong to the buyer (gate-thawed for NPCS).
    #[account(
        mut,
        constraint = buyer_asset.owner == buyer_escrow.owner @ RailsError::NotEscrowOwner,
        constraint = buyer_asset.mint == asset_mint.key() @ RailsError::AssetMismatch,
    )]
    pub buyer_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, address = session.fee_receiver @ RailsError::FeeReceiverMismatch)]
    pub fee_receiver: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, address = session.reserve @ RailsError::ReserveMismatch)]
    pub reserve: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = session.usdc_mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = session.asset_mint @ RailsError::AssetMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(fill_index: u32)]
pub struct SettleRofrFill<'info> {
    #[account(seeds = [RailsConfig::SEED], bump = config.bump)]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        mut,
        seeds = [CashSession::SEED, session.operator.as_ref(), &session.session_id.to_le_bytes()],
        bump = session.bump,
        has_one = operator @ RailsError::NotOperator
    )]
    pub session: Box<Account<'info, CashSession>>,
    #[account(mut)]
    pub operator: Signer<'info>,
    /// Issuer ROFR authority (session snapshot); pays the cash.
    pub issuer: Signer<'info>,
    #[account(
        init,
        payer = operator,
        space = 8 + FillReceipt::INIT_SPACE,
        seeds = [FillReceipt::SEED, session.key().as_ref(), &fill_index.to_le_bytes()],
        bump
    )]
    pub receipt: Box<Account<'info, FillReceipt>>,
    #[account(
        mut,
        seeds = [Escrow::SEED, session.key().as_ref(), seller_escrow.owner.as_ref()],
        bump = seller_escrow.bump,
        has_one = session
    )]
    pub seller_escrow: Box<Account<'info, Escrow>>,
    #[account(mut, address = seller_escrow.asset_vault)]
    pub seller_asset_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        constraint = seller_usdc.owner == seller_escrow.owner @ RailsError::NotEscrowOwner,
        constraint = seller_usdc.mint == usdc_mint.key() @ RailsError::MintMismatch,
    )]
    pub seller_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Issuer's USDC (token program enforces the issuer's authority).
    #[account(
        mut,
        constraint = issuer_usdc.mint == usdc_mint.key() @ RailsError::MintMismatch,
    )]
    pub issuer_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Asset destination: must belong to the issuer.
    #[account(
        mut,
        constraint = issuer_asset.owner == issuer.key() @ RailsError::NotEscrowOwner,
        constraint = issuer_asset.mint == asset_mint.key() @ RailsError::AssetMismatch,
    )]
    pub issuer_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, address = session.fee_receiver @ RailsError::FeeReceiverMismatch)]
    pub fee_receiver: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, address = session.reserve @ RailsError::ReserveMismatch)]
    pub reserve: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = session.usdc_mint @ RailsError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = session.asset_mint @ RailsError::AssetMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}
