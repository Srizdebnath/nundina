//! `liquidity-facility` — advances against pledged collateral, health
//! checks, and the liquidation ladder for permissioned collateral.
//!
//! SRS 08 §4.5 (LF-P1..P6) · doc 06 §5 · #139. Audit-scope-critical.
//!
//! The engine owns the *semantics*; `payment-rails` moves the cash (the
//! facility fund, advances, repayments) and `auction-engine` runs the
//! forced sale. Collateral sits in a per-position vault owned by this
//! program's signer PDA.
//!
//! - LF-P1 `pledge_and_advance`: collateral is valued at mark-engine's
//!   `MARK_PROTECTIVE` (never raw NAV), the advance rate is ≤ the facility's
//!   curator rate ≤ the H1 hard ceiling (`credit_math::MAX_ADVANCE_RATE_
//!   BPS`), and new advances need a **Fresh** mark (Warned blocks them).
//! - LF-P2 `open_facility`: a rails facility fund whose curator is this
//!   program's signer PDA (rails `EngineGrant`, role FACILITY); anyone funds
//!   it through rails `fund_facility`.
//! - LF-P3 `check_health`: debt (rails principal + interest accrued to now)
//!   over collateral value at `MARK_PROTECTIVE`; above `maintenance_ltv`
//!   the position is Breached; a top-up or repayment cures it.
//! - LF-P4 / H4 `start_liquidation`, every ladder step and the forced sale
//!   re-read the mark: **no liquidation step runs on a Stale or Suspended
//!   mark** (`Feed::allows_liquidation`), and only after the grace window.
//! - LF-P5 ladder, in order, each step with its own window:
//!   (1) issuer ROFR at `MARK_PROTECTIVE` → (2) the pre-registered backstop
//!   at its contracted haircut → (3) a forced-sale auction-engine session
//!   (`forced_sale = true`, flagged print, excluded from marks) → (4)
//!   workout: unsold collateral to the lender's workout account. Proceeds
//!   repay the rails advance first, the surplus goes to the borrower, and a
//!   shortfall is booked and evented (non-recourse).
//! - LF-P6 interest accrues per second in the rails (`credit_math::
//!   accrue_interest` at each touch); `repay` is the rails' own.

#![deny(unsafe_code)]
#![allow(missing_docs)]
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::{system_program, InstructionData};
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};
use auction_engine::{Phase as AuctionPhase, Session as AuctionSession, Side};
use credit_math::{mul_div, MAX_ADVANCE_RATE_BPS};
use mark_engine::Feed;

declare_id!("PTNCoDA5WTbAN5RARzUA19Yr3qJYBzwwH63RPFKS7hY");

pub const SIGNER_SEED: &[u8] = b"lf_signer";
pub const FACILITY_SEED: &[u8] = b"lf_facility";
pub const POSITION_SEED: &[u8] = b"position";
pub const COLLATERAL_SEED: &[u8] = b"collateral";
pub const PROCEEDS_SEED: &[u8] = b"proceeds";
/// Ceiling on the backstop bidder's contracted haircut.
pub const MAX_BACKSTOP_HAIRCUT_BPS: u64 = 5_000;
/// Lamports moved into the signer PDA for the rent of the accounts it
/// creates through the rails and the auction (facility, advance, escrow,
/// vaults, session); unused lamports stay as its float.
pub const SIGNER_FLOAT: u64 = 50_000_000;
/// The forced-sale ask's bond (micro-USDC), funded by whoever lists it.
pub const FORCED_SALE_BOND: u64 = 1;

const BPS: u64 = 10_000;

pub fn signer_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SIGNER_SEED], &crate::ID)
}

#[program]
pub mod liquidity_facility {
    use super::*;

    /// LF-P2: open a facility and its rails fund (curator = signer PDA).
    /// Remaining accounts: rails `OpenFacility` list + the signer's
    /// `EngineGrant`.
    pub fn open_facility<'info>(
        ctx: Context<'info, OpenFacility<'info>>,
        facility_id: u64,
        params: FacilityParams,
    ) -> Result<()> {
        params.validate()?;
        let signer = ctx.accounts.signer.key();
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 8, LfError::RailsAccounts);
        let rails_facility = Pubkey::find_program_address(
            &[
                payment_rails::Facility::SEED,
                signer.as_ref(),
                &facility_id.to_le_bytes(),
            ],
            &payment_rails::ID,
        )
        .0;
        require_keys_eq!(rails[1].key(), rails_facility, LfError::RailsAccounts);
        require_keys_eq!(rails[4].key(), signer, LfError::RailsAccounts);
        fund_signer(
            &ctx.accounts.authority,
            &ctx.accounts.signer,
            &ctx.accounts.system_program,
            SIGNER_FLOAT,
        )?;
        let bump = ctx.bumps.signer;
        forward(
            &ctx.accounts.rails_program,
            &payment_rails::ID,
            rails,
            payment_rails::instruction::OpenFacility {
                facility_id,
                rate_bps: params.rate_bps,
            }
            .data(),
            &signer,
            bump,
        )?;
        let cfg = &mut ctx.accounts.config;
        cfg.authority = ctx.accounts.authority.key();
        cfg.facility_id = facility_id;
        cfg.rails_facility = rails_facility;
        cfg.usdc_mint = ctx.accounts.usdc_mint.key();
        cfg.asset_mint = ctx.accounts.feed.asset_mint;
        cfg.asset_decimals = ctx.accounts.feed.asset_decimals;
        cfg.feed = ctx.accounts.feed.key();
        cfg.params = params;
        cfg.bump = ctx.bumps.config;
        cfg.signer_bump = bump;
        emit!(FacilityOpened {
            config: cfg.key(),
            rails_facility,
            asset_mint: cfg.asset_mint,
            params,
        });
        Ok(())
    }

    /// LF-P1: pledge `qty` of the asset and draw `amount` USDC. Remaining
    /// accounts: rails `AdvanceFunds` list.
    pub fn pledge_and_advance<'info>(
        ctx: Context<'info, PledgeAndAdvance<'info>>,
        qty: u64,
        amount: u64,
    ) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(qty > 0 && amount > 0, LfError::ZeroAmount);
        let cfg = &ctx.accounts.config;
        let feed = &ctx.accounts.feed;
        // LF-P4: Warned / Stale / Suspended marks block new advances.
        require!(feed.allows_new_advance(now), LfError::MarkNotFresh);
        let value = collateral_value(feed, qty, now)?;
        let max = mul_div(
            u128::from(value),
            u128::from(cfg.params.advance_rate_bps),
            u128::from(BPS),
        )
        .map_err(|_| LfError::MathOverflow)?;
        require!(u128::from(amount) <= max, LfError::AdvanceAboveRate);

        let borrower = ctx.accounts.borrower.key();
        let signer = ctx.accounts.signer.key();
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 11, LfError::RailsAccounts);
        require_keys_eq!(rails[1].key(), cfg.rails_facility, LfError::RailsAccounts);
        require_keys_eq!(rails[4].key(), borrower, LfError::RailsAccounts);
        require_keys_eq!(rails[8].key(), signer, LfError::RailsAccounts);

        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.asset_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.borrower_asset.to_account_info(),
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.collateral_vault.to_account_info(),
                    authority: ctx.accounts.borrower.to_account_info(),
                },
            ),
            qty,
            ctx.accounts.asset_mint.decimals,
        )?;
        let position_key = ctx.accounts.position.key();
        forward(
            &ctx.accounts.rails_program,
            &payment_rails::ID,
            rails,
            payment_rails::instruction::AdvanceFunds {
                amount,
                collateral_ref_hash: position_key.to_bytes(),
                collateral_value: value,
                advance_rate_bps: cfg.params.advance_rate_bps,
            }
            .data(),
            &signer,
            cfg.signer_bump,
        )?;
        let p = &mut ctx.accounts.position;
        p.config = cfg.key();
        p.borrower = borrower;
        p.qty = qty;
        p.collateral_vault = ctx.accounts.collateral_vault.key();
        p.proceeds = ctx.accounts.proceeds.key();
        p.advanced = amount;
        p.opened_at = now;
        p.state = PositionState::Active;
        p.bump = ctx.bumps.position;
        emit!(Pledged {
            position: position_key,
            borrower,
            qty,
            collateral_value: value,
            advanced: amount,
        });
        Ok(())
    }

    /// Add collateral (cures a breach at the next `check_health`).
    pub fn add_collateral(ctx: Context<AddCollateral>, qty: u64) -> Result<()> {
        require!(qty > 0, LfError::ZeroAmount);
        let p = &mut ctx.accounts.position;
        require!(
            matches!(p.state, PositionState::Active | PositionState::Breached),
            LfError::WrongState
        );
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.asset_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.borrower_asset.to_account_info(),
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.collateral_vault.to_account_info(),
                    authority: ctx.accounts.borrower.to_account_info(),
                },
            ),
            qty,
            ctx.accounts.asset_mint.decimals,
        )?;
        p.qty = p.qty.checked_add(qty).ok_or(LfError::MathOverflow)?;
        emit!(CollateralAdded {
            position: p.key(),
            qty,
            total: p.qty,
        });
        Ok(())
    }

    /// LF-P3: debt over value at `MARK_PROTECTIVE` against the maintenance
    /// LTV. Breach starts the grace clock; a healthy reading cures it.
    /// Permissionless.
    pub fn check_health(ctx: Context<CheckHealth>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        let debt = owed_now(&ctx.accounts.rails_facility, &ctx.accounts.advance, now)?;
        let value = collateral_value(&ctx.accounts.feed, ctx.accounts.position.qty, now)?;
        let breached = is_breached(debt, value, cfg.params.maintenance_ltv_bps);
        let p = &mut ctx.accounts.position;
        match (p.state, breached) {
            (PositionState::Active, true) => {
                p.state = PositionState::Breached;
                p.breach_ts = now;
            }
            (PositionState::Breached, false) => {
                p.state = PositionState::Active;
                p.breach_ts = 0;
            }
            (PositionState::Active | PositionState::Breached, _) => {}
            _ => return err!(LfError::WrongState),
        }
        emit!(HealthChecked {
            position: p.key(),
            debt,
            collateral_value: value,
            ltv_bps: ltv_bps(debt, value),
            maintenance_ltv_bps: cfg.params.maintenance_ltv_bps,
            mark_state: ctx.accounts.feed.state_at(now),
            state: p.state,
        });
        Ok(())
    }

    /// LF-P4/P5: past the grace window and still breached at a Fresh or
    /// Warned mark, the ladder starts at the issuer's ROFR. Permissionless.
    pub fn start_liquidation(ctx: Context<CheckHealth>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        let feed = &ctx.accounts.feed;
        require!(feed.allows_liquidation(now), LfError::StaleMark);
        let p = &ctx.accounts.position;
        require!(p.state == PositionState::Breached, LfError::WrongState);
        require!(
            now >= p.breach_ts.saturating_add(cfg.params.grace_seconds),
            LfError::GraceActive
        );
        let debt = owed_now(&ctx.accounts.rails_facility, &ctx.accounts.advance, now)?;
        let value = collateral_value(feed, p.qty, now)?;
        require!(
            is_breached(debt, value, cfg.params.maintenance_ltv_bps),
            LfError::NotBreached
        );
        let deadline = now.saturating_add(cfg.params.issuer_window);
        let p = &mut ctx.accounts.position;
        step(p, PositionState::IssuerRofr, deadline, now);
        Ok(())
    }

    /// LF-P5 steps 1–2: the issuer (at `MARK_PROTECTIVE`) or the backstop
    /// (at its contracted haircut) buys the whole collateral. The price
    /// repays the rails advance first; the surplus goes to the borrower; a
    /// shortfall is booked. Remaining accounts: rails `Repay` list.
    pub fn ladder_buy<'info>(ctx: Context<'info, LadderBuy<'info>>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        let feed = &ctx.accounts.feed;
        require!(feed.allows_liquidation(now), LfError::StaleMark);
        let p = &ctx.accounts.position;
        require!(now < p.step_deadline, LfError::WindowClosed);
        let buyer = ctx.accounts.buyer.key();
        let value = collateral_value(feed, p.qty, now)?;
        let price = match p.state {
            PositionState::IssuerRofr => {
                require_keys_eq!(buyer, cfg.params.issuer, LfError::NotLadderBuyer);
                value
            }
            PositionState::Backstop => {
                require_keys_eq!(buyer, cfg.params.backstop, LfError::NotLadderBuyer);
                u64::try_from(
                    mul_div(
                        u128::from(value),
                        u128::from(BPS - cfg.params.backstop_haircut_bps),
                        u128::from(BPS),
                    )
                    .map_err(|_| LfError::MathOverflow)?,
                )
                .map_err(|_| LfError::MathOverflow)?
            }
            _ => return err!(LfError::WrongState),
        };
        let qty = p.qty;
        let state = p.state;
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.buyer_usdc.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.proceeds.to_account_info(),
                    authority: ctx.accounts.buyer.to_account_info(),
                },
            ),
            price,
            ctx.accounts.usdc_mint.decimals,
        )?;
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[cfg.signer_bump]];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.asset_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.buyer_asset.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ),
            qty,
            ctx.accounts.asset_mint.decimals,
        )?;
        let accts = Proceeds {
            rails_program: &ctx.accounts.rails_program,
            signer: &ctx.accounts.signer,
            proceeds: &mut ctx.accounts.proceeds,
            borrower_usdc: &ctx.accounts.borrower_usdc,
            usdc_mint: &ctx.accounts.usdc_mint,
            token_program: &ctx.accounts.token_program,
        };
        let r = apply_proceeds(cfg, accts, ctx.remaining_accounts, now)?;
        let p = &mut ctx.accounts.position;
        p.qty = 0;
        p.shortfall = r.shortfall;
        emit!(LadderSale {
            position: p.key(),
            step: state,
            buyer,
            qty,
            price,
            repaid: r.repaid,
            to_borrower: r.surplus,
            shortfall: r.shortfall,
        });
        step(p, PositionState::Closed, 0, now);
        Ok(())
    }

    /// LF-P5: pass the current step — its buyer declines, or anyone once
    /// its window has closed. ROFR → backstop → forced sale.
    pub fn pass_step(ctx: Context<PassStep>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        let caller = ctx.accounts.caller.key();
        let p = &mut ctx.accounts.position;
        let (owner, next, window) = match p.state {
            PositionState::IssuerRofr => (
                cfg.params.issuer,
                PositionState::Backstop,
                cfg.params.backstop_window,
            ),
            PositionState::Backstop => (cfg.params.backstop, PositionState::ForcedSale, 0),
            _ => return err!(LfError::WrongState),
        };
        require!(
            caller == owner || now >= p.step_deadline,
            LfError::NotLadderBuyer
        );
        step(p, next, now.saturating_add(window), now);
        Ok(())
    }

    /// LF-P5 step 3a: open the forced-sale auction session (the signer PDA
    /// is the venue's forced-sale opener). Remaining accounts: the
    /// auction-engine `OpenSession` list followed by its rails list.
    pub fn open_forced_sale<'info>(
        ctx: Context<'info, ForcedSale<'info>>,
        session_id: u64,
        schedule: ForcedSchedule,
    ) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        require!(
            ctx.accounts.feed.allows_liquidation(now),
            LfError::StaleMark
        );
        let p = &ctx.accounts.position;
        require!(p.state == PositionState::ForcedSale, LfError::WrongState);
        require!(p.auction_session == Pubkey::default(), LfError::WrongState);
        let signer = ctx.accounts.signer.key();
        let accts = ctx.remaining_accounts;
        require!(accts.len() == 13, LfError::RailsAccounts);
        require_keys_eq!(accts[4].key(), signer, LfError::RailsAccounts);
        fund_signer(
            &ctx.accounts.caller,
            &ctx.accounts.signer,
            &ctx.accounts.system_program,
            SIGNER_FLOAT,
        )?;
        let params = auction_engine::SessionParams {
            commit_end: schedule.commit_end,
            reveal_end: schedule.reveal_end,
            rofr_window: 0,
            settle_deadline: schedule.settle_deadline,
            bond: FORCED_SALE_BOND,
            max_cash: cfg.params.forced_sale_max_cash,
            min_reveals: 2,
            max_discount_bps: 0,
            forced_sale: true,
            rofr_authority: Pubkey::default(),
            disclosure_hash: p.key().to_bytes(),
        };
        forward(
            &ctx.accounts.auction_program,
            &auction_engine::ID,
            accts,
            auction_engine::instruction::OpenSession { session_id, params }.data(),
            &signer,
            cfg.signer_bump,
        )?;
        let p = &mut ctx.accounts.position;
        p.auction_session = accts[1].key();
        emit!(ForcedSaleOpened {
            position: p.key(),
            auction_session: p.auction_session,
            qty: p.qty,
        });
        Ok(())
    }

    /// LF-P5 step 3b: list the whole collateral as the session's ask at the
    /// lowest limit (a forced sale has no reserve). The caller funds the
    /// ask bond. Remaining accounts: rails `OpenAssetVault` (8), then the
    /// auction-engine `Commit` list (3) and its rails `DepositAsk` list (13).
    pub fn list_forced_sale<'info>(ctx: Context<'info, ListForcedSale<'info>>) -> Result<()> {
        let cfg = &ctx.accounts.config;
        let p = &ctx.accounts.position;
        require!(p.state == PositionState::ForcedSale, LfError::WrongState);
        let signer = ctx.accounts.signer.key();
        let accts = ctx.remaining_accounts;
        require!(accts.len() == 24, LfError::RailsAccounts);
        require_keys_eq!(accts[8].key(), p.auction_session, LfError::RailsAccounts);
        require_keys_eq!(accts[5].key(), signer, LfError::RailsAccounts);
        require_keys_eq!(accts[9].key(), signer, LfError::RailsAccounts);
        require_keys_eq!(accts[18].key(), p.proceeds, LfError::RailsAccounts);
        require_keys_eq!(accts[19].key(), p.collateral_vault, LfError::RailsAccounts);
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.caller_usdc.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.proceeds.to_account_info(),
                    authority: ctx.accounts.caller.to_account_info(),
                },
            ),
            FORCED_SALE_BOND,
            ctx.accounts.usdc_mint.decimals,
        )?;
        forward(
            &ctx.accounts.rails_program,
            &payment_rails::ID,
            &accts[..8],
            payment_rails::instruction::OpenAssetVault {}.data(),
            &signer,
            cfg.signer_bump,
        )?;
        let nonce = p.key().to_bytes();
        let commitment = auction_engine::commitment(Side::Sell, 1, p.qty, &nonce, &signer);
        forward(
            &ctx.accounts.auction_program,
            &auction_engine::ID,
            &accts[8..],
            auction_engine::instruction::CommitAsk {
                commitment_hash: commitment,
                qty: p.qty,
            }
            .data(),
            &signer,
            cfg.signer_bump,
        )?;
        Ok(())
    }

    /// LF-P5 step 3c: reveal the forced-sale ask (permissionless, inside
    /// the reveal window). Remaining accounts: the auction-engine `Reveal`
    /// list followed by its rails list.
    pub fn reveal_forced_sale<'info>(ctx: Context<'info, RevealForcedSale<'info>>) -> Result<()> {
        let cfg = &ctx.accounts.config;
        let p = &ctx.accounts.position;
        require!(p.state == PositionState::ForcedSale, LfError::WrongState);
        let signer = ctx.accounts.signer.key();
        let accts = ctx.remaining_accounts;
        require!(accts.len() == 8, LfError::RailsAccounts);
        require_keys_eq!(accts[1].key(), p.auction_session, LfError::RailsAccounts);
        require_keys_eq!(accts[2].key(), signer, LfError::RailsAccounts);
        forward(
            &ctx.accounts.auction_program,
            &auction_engine::ID,
            accts,
            auction_engine::instruction::Reveal {
                limit: 1,
                qty: p.qty,
                nonce: p.key().to_bytes(),
            }
            .data(),
            &signer,
            cfg.signer_bump,
        )
    }

    /// LF-P5 steps 3d + 4: once the forced-sale session has printed (or
    /// failed) and the rails have refunded the ask escrow, apply the
    /// proceeds (advance first, surplus to the borrower, shortfall booked)
    /// and assign any unsold collateral to the lender's workout account.
    /// Remaining accounts: rails `Repay` list.
    pub fn settle_forced_sale<'info>(ctx: Context<'info, SettleForcedSale<'info>>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        let p = &ctx.accounts.position;
        require!(p.state == PositionState::ForcedSale, LfError::WrongState);
        let session = &ctx.accounts.auction_session;
        require_keys_eq!(session.key(), p.auction_session, LfError::RailsAccounts);
        require!(
            matches!(session.phase, AuctionPhase::Printed | AuctionPhase::Failed),
            LfError::SaleNotDone
        );
        // The ask escrow must be refunded (bond + unsold asset back home).
        require!(
            ctx.accounts.ask_escrow.data_is_empty(),
            LfError::SaleNotDone
        );
        let sold = if session.phase == AuctionPhase::Printed {
            session.cleared_volume
        } else {
            0
        };
        let accts = Proceeds {
            rails_program: &ctx.accounts.rails_program,
            signer: &ctx.accounts.signer,
            proceeds: &mut ctx.accounts.proceeds,
            borrower_usdc: &ctx.accounts.borrower_usdc,
            usdc_mint: &ctx.accounts.usdc_mint,
            token_program: &ctx.accounts.token_program,
        };
        let r = apply_proceeds(cfg, accts, ctx.remaining_accounts, now)?;
        let unsold = ctx.accounts.collateral_vault.amount;
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[cfg.signer_bump]];
        if unsold > 0 {
            token_interface::transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.asset_token_program.key(),
                    TransferChecked {
                        from: ctx.accounts.collateral_vault.to_account_info(),
                        mint: ctx.accounts.asset_mint.to_account_info(),
                        to: ctx.accounts.workout_asset.to_account_info(),
                        authority: ctx.accounts.signer.to_account_info(),
                    },
                    &[seeds],
                ),
                unsold,
                ctx.accounts.asset_mint.decimals,
            )?;
        }
        let p = &mut ctx.accounts.position;
        p.qty = 0;
        p.shortfall = r.shortfall;
        emit!(ForcedSaleSettled {
            position: p.key(),
            sold,
            price: session.clear_price,
            repaid: r.repaid,
            to_borrower: r.surplus,
            shortfall: r.shortfall,
            to_workout: unsold,
        });
        let next = if unsold > 0 {
            PositionState::Workout
        } else {
            PositionState::Closed
        };
        step(p, next, 0, now);
        Ok(())
    }

    /// The advance is fully repaid (rails closed it): collateral goes back
    /// to the borrower and the position closes. Permissionless.
    pub fn release(ctx: Context<Release>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        let p = &ctx.accounts.position;
        require!(
            matches!(p.state, PositionState::Active | PositionState::Breached),
            LfError::WrongState
        );
        require!(ctx.accounts.advance.data_is_empty(), LfError::StillOwed);
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[cfg.signer_bump]];
        let qty = ctx.accounts.collateral_vault.amount;
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.asset_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.borrower_asset.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ),
            qty,
            ctx.accounts.asset_mint.decimals,
        )?;
        let p = &mut ctx.accounts.position;
        p.qty = 0;
        emit!(Released {
            position: p.key(),
            qty,
        });
        step(p, PositionState::Closed, 0, now);
        Ok(())
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

/// Collateral value at `MARK_PROTECTIVE` (micro-USDC, floored).
pub fn collateral_value(feed: &Feed, qty: u64, now: i64) -> Result<u64> {
    let price = feed.pico_to_price(feed.mark_protective(now)?)?;
    let v = mul_div(
        u128::from(qty),
        u128::from(price),
        10u128.pow(u32::from(feed.asset_decimals)),
    )
    .map_err(|_| LfError::MathOverflow)?;
    u64::try_from(v).map_err(|_| error!(LfError::MathOverflow))
}

/// LTV in bps, rounded up (against the borrower); no value ⇒ max.
pub fn ltv_bps(debt: u64, value: u64) -> u64 {
    if value == 0 {
        return u64::MAX;
    }
    let v = (u128::from(debt) * u128::from(BPS)).div_ceil(u128::from(value));
    u64::try_from(v).unwrap_or(u64::MAX)
}

pub fn is_breached(debt: u64, value: u64, maintenance_ltv_bps: u64) -> bool {
    debt > 0 && ltv_bps(debt, value) > maintenance_ltv_bps
}

/// What the rails would collect now (interest accrued exactly as the rails
/// accrue it at their next touch).
fn owed_now(
    facility: &Account<payment_rails::Facility>,
    advance: &Account<payment_rails::Advance>,
    now: i64,
) -> Result<u64> {
    let mut a: payment_rails::Advance = (**advance).clone();
    a.accrue(facility.rate_bps, now)?;
    Ok(a.owed())
}

fn step(p: &mut Position, to: PositionState, deadline: i64, now: i64) {
    let from = p.state;
    p.state = to;
    p.step_deadline = deadline;
    emit!(LadderStep {
        position: p.borrower,
        from,
        to,
        deadline,
        at: now,
    });
}

fn fund_signer<'info>(
    from: &Signer<'info>,
    signer: &UncheckedAccount<'info>,
    system: &Program<'info, System>,
    lamports: u64,
) -> Result<()> {
    system_program::transfer(
        CpiContext::new(
            system.key(),
            system_program::Transfer {
                from: from.to_account_info(),
                to: signer.to_account_info(),
            },
        ),
        lamports,
    )
}

/// Forward `accts` to `program_id` with `data`, the signer PDA signing.
fn forward<'info>(
    program: &AccountInfo<'info>,
    program_id: &Pubkey,
    accts: &[AccountInfo<'info>],
    data: Vec<u8>,
    signer: &Pubkey,
    bump: u8,
) -> Result<()> {
    require_keys_eq!(program.key(), *program_id, LfError::RailsAccounts);
    let metas = accts
        .iter()
        .map(|a| AccountMeta {
            pubkey: a.key(),
            is_signer: a.is_signer || a.key() == *signer,
            is_writable: a.is_writable,
        })
        .collect();
    let mut infos = accts.to_vec();
    infos.push(program.clone());
    invoke_signed(
        &Instruction {
            program_id: *program_id,
            accounts: metas,
            data,
        },
        &infos,
        &[&[SIGNER_SEED, &[bump]]],
    )
    .map_err(Into::into)
}

struct Proceeds<'a, 'info> {
    rails_program: &'a UncheckedAccount<'info>,
    signer: &'a UncheckedAccount<'info>,
    proceeds: &'a mut Box<InterfaceAccount<'info, TokenAccount>>,
    borrower_usdc: &'a InterfaceAccount<'info, TokenAccount>,
    usdc_mint: &'a InterfaceAccount<'info, Mint>,
    token_program: &'a Interface<'info, TokenInterface>,
}

struct Applied {
    repaid: u64,
    surplus: u64,
    shortfall: u64,
}

/// LF-P5 waterfall of a liquidation's proceeds: the rails advance first
/// (rails `repay`, paid by the signer from the position's proceeds
/// account), the rest to the borrower; any debt left is the shortfall
/// (non-recourse, evented by the caller). `rails`: the rails `Repay` list.
fn apply_proceeds<'info>(
    cfg: &FacilityConfig,
    a: Proceeds<'_, 'info>,
    rails: &[AccountInfo<'info>],
    now: i64,
) -> Result<Applied> {
    require!(rails.len() == 8, LfError::RailsAccounts);
    require_keys_eq!(rails[0].key(), cfg.rails_facility, LfError::RailsAccounts);
    require_keys_eq!(rails[4].key(), a.proceeds.key(), LfError::RailsAccounts);
    require_keys_eq!(rails[5].key(), a.signer.key(), LfError::RailsAccounts);
    // The sale has just credited this account in the same instruction.
    a.proceeds.reload()?;
    let available = a.proceeds.amount;
    let owed = if rails[1].data_is_empty() {
        0
    } else {
        let facility =
            payment_rails::Facility::try_deserialize(&mut &rails[0].try_borrow_data()?[..])?;
        let mut adv =
            payment_rails::Advance::try_deserialize(&mut &rails[1].try_borrow_data()?[..])?;
        adv.accrue(facility.rate_bps, now)?;
        adv.owed()
    };
    let repaid = available.min(owed);
    if repaid > 0 {
        forward(
            a.rails_program,
            &payment_rails::ID,
            rails,
            payment_rails::instruction::Repay { amount: repaid }.data(),
            &a.signer.key(),
            cfg.signer_bump,
        )?;
    }
    a.proceeds.reload()?;
    let surplus = a.proceeds.amount;
    if surplus > 0 {
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[cfg.signer_bump]];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                a.token_program.key(),
                TransferChecked {
                    from: a.proceeds.to_account_info(),
                    mint: a.usdc_mint.to_account_info(),
                    to: a.borrower_usdc.to_account_info(),
                    authority: a.signer.to_account_info(),
                },
                &[seeds],
            ),
            surplus,
            a.usdc_mint.decimals,
        )?;
    }
    Ok(Applied {
        repaid,
        surplus,
        shortfall: owed - repaid,
    })
}

// ── state ───────────────────────────────────────────────────────────────

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct FacilityParams {
    /// Interest APR (rails `rate_bps`, ≤ rails `MAX_RATE_BPS`).
    pub rate_bps: u64,
    /// Curator advance rate (≤ H1 `MAX_ADVANCE_RATE_BPS`).
    pub advance_rate_bps: u64,
    /// Breach when debt / value exceeds this (> advance rate, ≤ 100%).
    pub maintenance_ltv_bps: u64,
    /// Top-up window between breach and the ladder (LF-P3).
    pub grace_seconds: i64,
    /// Ladder step 1: the issuer (ROFR at MARK_PROTECTIVE).
    pub issuer: Pubkey,
    pub issuer_window: i64,
    /// Ladder step 2: the pre-registered backstop bidder.
    pub backstop: Pubkey,
    pub backstop_haircut_bps: u64,
    pub backstop_window: i64,
    /// Ladder step 3: per-investor cash cap of the forced-sale session.
    pub forced_sale_max_cash: u64,
    /// Ladder step 4: owner of the lender's workout asset account.
    pub lender_workout: Pubkey,
}

impl FacilityParams {
    pub fn validate(&self) -> Result<()> {
        require!(
            self.advance_rate_bps > 0 && self.advance_rate_bps <= MAX_ADVANCE_RATE_BPS,
            LfError::AdvanceRateAboveCeiling
        );
        require!(
            self.maintenance_ltv_bps > self.advance_rate_bps && self.maintenance_ltv_bps <= BPS,
            LfError::BadParams
        );
        require!(
            self.backstop_haircut_bps <= MAX_BACKSTOP_HAIRCUT_BPS,
            LfError::BadParams
        );
        require!(
            self.grace_seconds >= 0
                && self.issuer_window > 0
                && self.backstop_window > 0
                && self.forced_sale_max_cash > 0,
            LfError::BadParams
        );
        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct ForcedSchedule {
    pub commit_end: i64,
    pub reveal_end: i64,
    pub settle_deadline: i64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum PositionState {
    Active,
    Breached,
    IssuerRofr,
    Backstop,
    ForcedSale,
    Closed,
    Workout,
}

#[account]
#[derive(InitSpace)]
pub struct FacilityConfig {
    pub authority: Pubkey,
    pub facility_id: u64,
    /// payment-rails `Facility` (curator = this program's signer PDA).
    pub rails_facility: Pubkey,
    pub usdc_mint: Pubkey,
    pub asset_mint: Pubkey,
    pub asset_decimals: u8,
    /// mark-engine feed of the collateral.
    pub feed: Pubkey,
    pub params: FacilityParams,
    pub bump: u8,
    pub signer_bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Position {
    pub config: Pubkey,
    pub borrower: Pubkey,
    /// Collateral held (0 once sold, released or worked out).
    pub qty: u64,
    pub collateral_vault: Pubkey,
    /// Liquidation proceeds (owner = signer PDA).
    pub proceeds: Pubkey,
    pub advanced: u64,
    pub opened_at: i64,
    pub state: PositionState,
    pub breach_ts: i64,
    /// Current ladder step's deadline.
    pub step_deadline: i64,
    pub auction_session: Pubkey,
    /// Debt left after the ladder (non-recourse).
    pub shortfall: u64,
    pub bump: u8,
}

// ── accounts ────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(facility_id: u64)]
pub struct OpenFacility<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + FacilityConfig::INIT_SPACE,
        seeds = [FACILITY_SEED, &facility_id.to_le_bytes()],
        bump
    )]
    pub config: Box<Account<'info, FacilityConfig>>,
    /// CHECK: the signer PDA (system account; rails curator).
    #[account(mut, seeds = [SIGNER_SEED], bump)]
    pub signer: UncheckedAccount<'info>,
    pub feed: Box<Account<'info, Feed>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub authority: Signer<'info>,
    /// CHECK: payment-rails (address-checked in `forward`).
    pub rails_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct PledgeAndAdvance<'info> {
    #[account(seeds = [FACILITY_SEED, &config.facility_id.to_le_bytes()], bump = config.bump)]
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(
        init,
        payer = borrower,
        space = 8 + Position::INIT_SPACE,
        seeds = [POSITION_SEED, config.key().as_ref(), borrower.key().as_ref()],
        bump
    )]
    pub position: Box<Account<'info, Position>>,
    #[account(
        init,
        payer = borrower,
        seeds = [COLLATERAL_SEED, position.key().as_ref()],
        bump,
        token::mint = asset_mint,
        token::authority = signer,
        token::token_program = asset_token_program,
    )]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        init,
        payer = borrower,
        seeds = [PROCEEDS_SEED, position.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = signer,
        token::token_program = token_program,
    )]
    pub proceeds: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = config.feed @ LfError::FeedMismatch)]
    pub feed: Box<Account<'info, Feed>>,
    /// CHECK: the signer PDA; signs the rails CPI.
    #[account(mut, seeds = [SIGNER_SEED], bump = config.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut, token::mint = asset_mint, token::authority = borrower)]
    pub borrower_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = config.asset_mint @ LfError::FeedMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = config.usdc_mint @ LfError::FeedMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    pub token_program: Interface<'info, TokenInterface>,
    /// CHECK: payment-rails (address-checked in `forward`).
    pub rails_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AddCollateral<'info> {
    #[account(
        mut,
        has_one = borrower @ LfError::NotBorrower,
        has_one = collateral_vault @ LfError::RailsAccounts
    )]
    pub position: Box<Account<'info, Position>>,
    #[account(mut)]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub borrower: Signer<'info>,
    #[account(mut, token::authority = borrower)]
    pub borrower_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = collateral_vault.mint @ LfError::FeedMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub asset_token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct CheckHealth<'info> {
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(mut, has_one = config @ LfError::RailsAccounts)]
    pub position: Box<Account<'info, Position>>,
    #[account(address = config.feed @ LfError::FeedMismatch)]
    pub feed: Box<Account<'info, Feed>>,
    #[account(address = config.rails_facility @ LfError::RailsAccounts)]
    pub rails_facility: Box<Account<'info, payment_rails::Facility>>,
    #[account(
        constraint = advance.facility == config.rails_facility @ LfError::RailsAccounts,
        constraint = advance.borrower == position.borrower @ LfError::RailsAccounts
    )]
    pub advance: Box<Account<'info, payment_rails::Advance>>,
}

#[derive(Accounts)]
pub struct LadderBuy<'info> {
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(
        mut,
        has_one = config @ LfError::RailsAccounts,
        has_one = collateral_vault @ LfError::RailsAccounts,
        has_one = proceeds @ LfError::RailsAccounts
    )]
    pub position: Box<Account<'info, Position>>,
    #[account(address = config.feed @ LfError::FeedMismatch)]
    pub feed: Box<Account<'info, Feed>>,
    /// CHECK: the signer PDA.
    #[account(mut, seeds = [SIGNER_SEED], bump = config.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    pub buyer: Signer<'info>,
    #[account(mut)]
    pub buyer_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::authority = buyer)]
    pub buyer_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub proceeds: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, constraint = borrower_usdc.owner == position.borrower @ LfError::NotBorrower)]
    pub borrower_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = config.asset_mint @ LfError::FeedMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = config.usdc_mint @ LfError::FeedMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    /// CHECK: payment-rails (address-checked in `forward`).
    pub rails_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct PassStep<'info> {
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(mut, has_one = config @ LfError::RailsAccounts)]
    pub position: Box<Account<'info, Position>>,
    pub caller: Signer<'info>,
}

#[derive(Accounts)]
pub struct ForcedSale<'info> {
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(mut, has_one = config @ LfError::RailsAccounts)]
    pub position: Box<Account<'info, Position>>,
    #[account(address = config.feed @ LfError::FeedMismatch)]
    pub feed: Box<Account<'info, Feed>>,
    /// CHECK: the signer PDA (the venue's forced-sale opener).
    #[account(mut, seeds = [SIGNER_SEED], bump = config.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub caller: Signer<'info>,
    /// CHECK: auction-engine (address-checked in `forward`).
    pub auction_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ListForcedSale<'info> {
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(has_one = config @ LfError::RailsAccounts, has_one = proceeds @ LfError::RailsAccounts)]
    pub position: Box<Account<'info, Position>>,
    /// CHECK: the signer PDA (the ask's owner).
    #[account(mut, seeds = [SIGNER_SEED], bump = config.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    pub caller: Signer<'info>,
    #[account(mut)]
    pub caller_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub proceeds: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = config.usdc_mint @ LfError::FeedMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
    /// CHECK: payment-rails (address-checked in `forward`).
    pub rails_program: UncheckedAccount<'info>,
    /// CHECK: auction-engine (address-checked in `forward`).
    pub auction_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct RevealForcedSale<'info> {
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(has_one = config @ LfError::RailsAccounts)]
    pub position: Box<Account<'info, Position>>,
    /// CHECK: the signer PDA (the ask's owner).
    #[account(seeds = [SIGNER_SEED], bump = config.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    /// CHECK: auction-engine (address-checked in `forward`).
    pub auction_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct SettleForcedSale<'info> {
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(
        mut,
        has_one = config @ LfError::RailsAccounts,
        has_one = collateral_vault @ LfError::RailsAccounts,
        has_one = proceeds @ LfError::RailsAccounts
    )]
    pub position: Box<Account<'info, Position>>,
    pub auction_session: Box<Account<'info, AuctionSession>>,
    /// CHECK: the signer's rails escrow in that session; must be closed.
    #[account(
        address = auction_engine::rails_escrow(&auction_session.rails_session, &signer.key())
            @ LfError::RailsAccounts
    )]
    pub ask_escrow: UncheckedAccount<'info>,
    /// CHECK: the signer PDA.
    #[account(mut, seeds = [SIGNER_SEED], bump = config.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub proceeds: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, constraint = borrower_usdc.owner == position.borrower @ LfError::NotBorrower)]
    pub borrower_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        constraint = workout_asset.owner == config.params.lender_workout @ LfError::NotWorkout
    )]
    pub workout_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = config.asset_mint @ LfError::FeedMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = config.usdc_mint @ LfError::FeedMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    /// CHECK: payment-rails (address-checked in `forward`).
    pub rails_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Release<'info> {
    pub config: Box<Account<'info, FacilityConfig>>,
    #[account(
        mut,
        has_one = config @ LfError::RailsAccounts,
        has_one = collateral_vault @ LfError::RailsAccounts
    )]
    pub position: Box<Account<'info, Position>>,
    /// CHECK: the rails advance PDA of this borrower; must be closed.
    #[account(
        address = Pubkey::find_program_address(
            &[payment_rails::Advance::SEED, config.rails_facility.as_ref(), position.borrower.as_ref()],
            &payment_rails::ID,
        ).0 @ LfError::RailsAccounts
    )]
    pub advance: UncheckedAccount<'info>,
    /// CHECK: the signer PDA.
    #[account(seeds = [SIGNER_SEED], bump = config.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, constraint = borrower_asset.owner == position.borrower @ LfError::NotBorrower)]
    pub borrower_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = config.asset_mint @ LfError::FeedMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub asset_token_program: Interface<'info, TokenInterface>,
}

// ── events ──────────────────────────────────────────────────────────────

#[event]
pub struct FacilityOpened {
    pub config: Pubkey,
    pub rails_facility: Pubkey,
    pub asset_mint: Pubkey,
    pub params: FacilityParams,
}

#[event]
pub struct Pledged {
    pub position: Pubkey,
    pub borrower: Pubkey,
    pub qty: u64,
    pub collateral_value: u64,
    pub advanced: u64,
}

#[event]
pub struct CollateralAdded {
    pub position: Pubkey,
    pub qty: u64,
    pub total: u64,
}

#[event]
pub struct HealthChecked {
    pub position: Pubkey,
    pub debt: u64,
    pub collateral_value: u64,
    pub ltv_bps: u64,
    pub maintenance_ltv_bps: u64,
    pub mark_state: mark_engine::MarkState,
    pub state: PositionState,
}

#[event]
pub struct LadderStep {
    /// The borrower of the position.
    pub position: Pubkey,
    pub from: PositionState,
    pub to: PositionState,
    pub deadline: i64,
    pub at: i64,
}

#[event]
pub struct LadderSale {
    pub position: Pubkey,
    pub step: PositionState,
    pub buyer: Pubkey,
    pub qty: u64,
    pub price: u64,
    pub repaid: u64,
    pub to_borrower: u64,
    pub shortfall: u64,
}

#[event]
pub struct ForcedSaleOpened {
    pub position: Pubkey,
    pub auction_session: Pubkey,
    pub qty: u64,
}

#[event]
pub struct ForcedSaleSettled {
    pub position: Pubkey,
    pub sold: u64,
    pub price: u64,
    pub repaid: u64,
    pub to_borrower: u64,
    pub shortfall: u64,
    pub to_workout: u64,
}

#[event]
pub struct Released {
    pub position: Pubkey,
    pub qty: u64,
}

// ── errors (append-only) ────────────────────────────────────────────────

#[error_code]
pub enum LfError {
    #[msg("advance rate above the H1 hard ceiling")]
    AdvanceRateAboveCeiling,
    #[msg("facility parameters out of bounds")]
    BadParams,
    #[msg("amount must be positive")]
    ZeroAmount,
    #[msg("rails / auction accounts do not match")]
    RailsAccounts,
    #[msg("feed or mint does not match the facility")]
    FeedMismatch,
    #[msg("new advances need a Fresh mark")]
    MarkNotFresh,
    #[msg("advance above collateral value x advance rate")]
    AdvanceAboveRate,
    #[msg("no liquidation on a Stale or Suspended mark (H4)")]
    StaleMark,
    #[msg("position is not in the state this step needs")]
    WrongState,
    #[msg("grace window still running")]
    GraceActive,
    #[msg("position is not breached at the current mark")]
    NotBreached,
    #[msg("ladder step window has closed")]
    WindowClosed,
    #[msg("signer is not this ladder step's buyer")]
    NotLadderBuyer,
    #[msg("forced sale not finished or ask escrow not refunded")]
    SaleNotDone,
    #[msg("advance still owed")]
    StillOwed,
    #[msg("signer / account is not the borrower's")]
    NotBorrower,
    #[msg("workout account not owned by the lender's workout key")]
    NotWorkout,
    #[msg("arithmetic overflow")]
    MathOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ltv_rounds_against_the_borrower() {
        assert_eq!(ltv_bps(5_000, 10_000), 5_000);
        assert_eq!(ltv_bps(1, 3), 3_334);
        assert_eq!(ltv_bps(1, 0), u64::MAX);
        assert!(is_breached(6_501, 10_000, 6_500));
        assert!(!is_breached(6_500, 10_000, 6_500));
        assert!(!is_breached(0, 0, 6_500), "nothing owed is never breached");
    }

    #[test]
    fn params_bounds() {
        let p = FacilityParams {
            rate_bps: 1_200,
            advance_rate_bps: 5_000,
            maintenance_ltv_bps: 6_500,
            grace_seconds: 3_600,
            issuer: Pubkey::new_unique(),
            issuer_window: 60,
            backstop: Pubkey::new_unique(),
            backstop_haircut_bps: 1_500,
            backstop_window: 60,
            forced_sale_max_cash: 1,
            lender_workout: Pubkey::new_unique(),
        };
        assert!(p.validate().is_ok());
        let mut q = p;
        q.advance_rate_bps = MAX_ADVANCE_RATE_BPS + 1;
        q.maintenance_ltv_bps = 9_000;
        assert!(q.validate().is_err(), "H1 ceiling");
        let mut q = p;
        q.maintenance_ltv_bps = q.advance_rate_bps;
        assert!(q.validate().is_err());
        let mut q = p;
        q.backstop_haircut_bps = MAX_BACKSTOP_HAIRCUT_BPS + 1;
        assert!(q.validate().is_err());
    }
}
