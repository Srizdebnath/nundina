//! `auction-engine` — sealed-bid, uniform-price batch auction sessions.
//!
//! SRS 08 §4.2 (AE-P1..P11) · doc 06 §2 · #136. Audit-scope-critical for
//! its escrow paths, which are all payment-rails CPIs: this program never
//! holds a token. It owns the *semantics* (phases, sealed commitments,
//! reveal checks, clearing, allocation, ROFR, the print); `payment-rails`
//! moves every micro-USDC and every asset unit, and CPIs credit-gate's
//! `settle_check` on every fill.
//!
//! Lifecycle (AE-P2), every transition evented:
//!
//! ```text
//! Commit ─(commit_end)→ Reveal ─(reveal_end, clear)→ Allocating ─→ IssuerRofr ─→ Settling ─(finish)→ Printed
//!    └──────────────── Failed (too few reveals · no cross ≥ reserve · suspended mark · settle deadline) ┘
//! ```
//!
//! - AE-P1 `open_session` snapshots every parameter into the [`Session`]
//!   and opens the rails `CashSession` (deadlines, bond, cap, ROFR) with the
//!   engine's operator PDA as its operator.
//! - AE-P3 `commit_bid` / `commit_ask`: a sealed
//!   `sha256(side ‖ limit ‖ qty ‖ nonce ‖ owner)` plus a pre-funded rails
//!   escrow (cash + bond, or asset + bond) in the same instruction.
//! - AE-P4 `reveal`: preimage check, budget check against the escrow, then
//!   rails `mark_revealed`. Unrevealed escrows forfeit their bond in the
//!   rails (`forfeit_bond` / `refund`).
//! - AE-P5 `clear`: `credit_math::find_uniform_clear_price` over revealed
//!   orders, reserve floor `NAV_effective × (1 − max_discount)` and
//!   reference `MARK_PROTECTIVE` from mark-engine; no cross ⇒ Failed.
//! - AE-P6 `allocate`: pro-rata at the margin (`credit_math::allocate_
//!   pro_rata`), then deterministic buyer×seller pairing into fills, in
//!   resumable chunks (cursor in the session).
//! - AE-P7 `exercise_rofr` / `close_rofr`: the issuer takes (part of) a
//!   fill at the clearing price, settled at once by rails
//!   `settle_rofr_fill`; or declines.
//! - AE-P8 `settle`: one fill per instruction (crank-loopable,
//!   permissionless) through rails `settle_fill`.
//! - AE-P9 `finish`: rails `finish_session`, then the `ClearingPrint` into
//!   mark-engine `record_print` (signed by the print PDA).
//! - AE-P10 `expire`: past any hard deadline the session fails and every
//!   escrow is refundable through the rails' permissionless refunds.
//! - AE-P11 forced-sale sessions: only the venue's forced-sale opener
//!   (liquidity-facility's PDA) may open them; no reserve; the print is
//!   flagged so mark-engine excludes it.

#![deny(unsafe_code)]
#![allow(missing_docs)]
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::{system_program, InstructionData};
use credit_math::{allocate_pro_rata, find_uniform_clear_price, mul_div, Order as MathOrder};
use mark_engine::{Feed, MarkState, PrintInput, PrintPolicy};

declare_id!("Cw6wNpVJUaofUDZewG8sDKZu3nnHga4GaJJvKn7rSod7");

/// Orders per session (fixed-capacity book, AE-P3 / G12).
pub const MAX_ORDERS: usize = 16;
/// Fills per session (buyer × seller pairs after allocation).
pub const MAX_FILLS: usize = 24;
/// H3 interim hard ceiling on the reserve discount below NAV_effective.
/// doc 06 §11 has no plan-of-record value yet (#123); this bounds what a
/// venue operator can configure until one is signed off.
pub const MAX_DISCOUNT_BPS: u64 = 5_000;
/// Lamports kept in the operator PDA so it stays a live system account.
pub const OPERATOR_FLOAT: u64 = 10_000_000;

const BPS: u64 = 10_000;

/// Seeds of the engine's signer PDAs.
pub const VENUE_SEED: &[u8] = b"venue";
pub const SESSION_SEED: &[u8] = b"session";
/// Signs every payment-rails call as the rails operator (`EngineGrant`).
pub const OPERATOR_SEED: &[u8] = b"rails_operator";
/// Signs mark-engine `record_print` (the feed's `print_source`).
pub const PRINT_SEED: &[u8] = b"print";

pub fn operator_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[OPERATOR_SEED], &crate::ID)
}

pub fn print_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[PRINT_SEED], &crate::ID)
}

/// The sealed commitment (AE-P3). Binding the owner stops a copied hash.
pub fn commitment(side: Side, limit: u64, qty: u64, nonce: &[u8; 32], owner: &Pubkey) -> [u8; 32] {
    solana_sha256_hasher::hashv(&[
        &[side as u8],
        &limit.to_le_bytes(),
        &qty.to_le_bytes(),
        nonce,
        owner.as_ref(),
    ])
    .to_bytes()
}

#[program]
pub mod auction_engine {
    use super::*;

    /// Create the venue. The signer is the venue operator authority (a
    /// Squads vault on devnet/mainnet); `forced_sale_opener` is the only
    /// key allowed to open forced-sale sessions (liquidity-facility's
    /// PDA, AE-P11). Seeds the operator PDA with its rent float.
    pub fn initialize_venue(
        ctx: Context<InitializeVenue>,
        forced_sale_opener: Pubkey,
    ) -> Result<()> {
        let (_, operator_bump) = operator_pda();
        let (_, print_bump) = print_pda();
        let venue = &mut ctx.accounts.venue;
        venue.authority = ctx.accounts.authority.key();
        venue.forced_sale_opener = forced_sale_opener;
        venue.sessions = 0;
        venue.bump = ctx.bumps.venue;
        venue.operator_bump = operator_bump;
        venue.print_bump = print_bump;
        top_up(
            &ctx.accounts.authority,
            &ctx.accounts.operator,
            &ctx.accounts.system_program,
            OPERATOR_FLOAT,
        )?;
        emit!(VenueConfigured {
            authority: venue.authority,
            forced_sale_opener,
        });
        Ok(())
    }

    /// Re-point the forced-sale opener (venue authority).
    pub fn set_forced_sale_opener(ctx: Context<VenueAdmin>, opener: Pubkey) -> Result<()> {
        let venue = &mut ctx.accounts.venue;
        venue.forced_sale_opener = opener;
        emit!(VenueConfigured {
            authority: venue.authority,
            forced_sale_opener: opener,
        });
        Ok(())
    }

    /// AE-P1: open a session. Remaining accounts: the rails `OpenSession`
    /// list (config, cash session, asset mint, operator, system program)
    /// followed by the operator's rails `EngineGrant`.
    pub fn open_session<'info>(
        ctx: Context<'info, OpenSession<'info>>,
        session_id: u64,
        params: SessionParams,
    ) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        params.validate(now)?;
        let opener = ctx.accounts.opener.key();
        let venue = &ctx.accounts.venue;
        if params.forced_sale {
            require_keys_eq!(
                opener,
                venue.forced_sale_opener,
                AuctionError::NotForcedSaleOpener
            );
        } else {
            require_keys_eq!(opener, venue.authority, AuctionError::NotVenueAuthority);
        }
        let feed = &ctx.accounts.feed;
        let asset_mint = feed.asset_mint;
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 6, AuctionError::RailsAccounts);
        let operator = ctx.accounts.operator.key();
        let expected_rails_session = Pubkey::find_program_address(
            &[
                payment_rails::CashSession::SEED,
                operator.as_ref(),
                &session_id.to_le_bytes(),
            ],
            &payment_rails::ID,
        )
        .0;
        require_keys_eq!(
            rails[1].key(),
            expected_rails_session,
            AuctionError::RailsAccounts
        );
        require_keys_eq!(rails[2].key(), asset_mint, AuctionError::AssetMismatch);
        require_keys_eq!(rails[3].key(), operator, AuctionError::RailsAccounts);

        // The operator PDA pays the CashSession rent: the opener funds it.
        let rent = Rent::get()?.minimum_balance(8 + payment_rails::CashSession::INIT_SPACE);
        top_up(
            &ctx.accounts.opener,
            &ctx.accounts.operator,
            &ctx.accounts.system_program,
            rent,
        )?;
        let data = payment_rails::instruction::OpenSession {
            session_id,
            params: payment_rails::SessionParams {
                commit_end: params.commit_end,
                reveal_end: params.reveal_end,
                settle_deadline: params.settle_deadline,
                bond_amount: params.bond,
                max_cash: params.max_cash,
                rofr_authority: params.rofr_authority,
            },
        }
        .data();
        rails_call(
            &ctx.accounts.rails_program,
            rails,
            data,
            &operator,
            ctx.accounts.venue.operator_bump,
        )?;

        let session = &mut ctx.accounts.session;
        session.venue = ctx.accounts.venue.key();
        session.session_id = session_id;
        session.asset_mint = asset_mint;
        session.asset_decimals = feed.asset_decimals;
        session.feed = feed.key();
        session.rails_session = expected_rails_session;
        session.params = params;
        session.phase = Phase::Commit;
        session.bump = ctx.bumps.session;
        let venue = &mut ctx.accounts.venue;
        venue.sessions = venue.sessions.saturating_add(1);
        emit!(PhaseChanged {
            session: session.key(),
            from: None,
            to: Phase::Commit,
            at: now,
        });
        Ok(())
    }

    /// AE-P3 buy side: record a sealed bid and escrow `cash` (+ the bond)
    /// in the rails. Remaining accounts: the rails `Deposit` list.
    pub fn commit_bid<'info>(
        ctx: Context<'info, Commit<'info>>,
        commitment_hash: [u8; 32],
        cash: u64,
    ) -> Result<()> {
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 9, AuctionError::RailsAccounts);
        require_keys_eq!(
            rails[6].key(),
            ctx.accounts.owner.key(),
            AuctionError::RailsAccounts
        );
        let data = payment_rails::instruction::Deposit { cash }.data();
        commit(ctx, Side::Buy, commitment_hash, cash, data)
    }

    /// AE-P3 sell side: record a sealed ask and escrow `qty` of the asset
    /// (+ the bond). The rails asset vault must already exist and, for a
    /// default-frozen asset, be gate-thawed. Remaining accounts: the rails
    /// `DepositAsk` list.
    pub fn commit_ask<'info>(
        ctx: Context<'info, Commit<'info>>,
        commitment_hash: [u8; 32],
        qty: u64,
    ) -> Result<()> {
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 13, AuctionError::RailsAccounts);
        require_keys_eq!(
            rails[9].key(),
            ctx.accounts.owner.key(),
            AuctionError::RailsAccounts
        );
        let data = payment_rails::instruction::DepositAsk { qty }.data();
        commit(ctx, Side::Sell, commitment_hash, qty, data)
    }

    /// AE-P4: reveal the preimage; checked against the commitment and the
    /// escrowed amount, then reported to the rails (`mark_revealed`).
    /// Remaining accounts: rails session, the owner's escrow, operator.
    pub fn reveal<'info>(
        ctx: Context<'info, Reveal<'info>>,
        limit: u64,
        qty: u64,
        nonce: [u8; 32],
    ) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let owner = ctx.accounts.owner.key();
        let session_key = ctx.accounts.session.key();
        let session = &mut ctx.accounts.session;
        require!(
            now >= session.params.commit_end && now < session.params.reveal_end,
            AuctionError::OutsideWindow
        );
        require!(
            matches!(session.phase, Phase::Commit | Phase::Reveal),
            AuctionError::WrongPhase
        );
        if session.phase == Phase::Commit {
            set_phase(session_key, session, Phase::Reveal, now);
        }
        let decimals = session.asset_decimals;
        let order = session
            .orders
            .iter_mut()
            .find(|o| o.owner == owner)
            .ok_or(AuctionError::NoOrder)?;
        require!(!order.revealed, AuctionError::AlreadyRevealed);
        require!(limit > 0 && qty > 0, AuctionError::ZeroAmount);
        require!(
            commitment(order.side, limit, qty, &nonce, &owner) == order.commitment,
            AuctionError::CommitmentMismatch
        );
        match order.side {
            Side::Buy => require!(
                gross(qty, limit, decimals)? <= order.escrowed,
                AuctionError::BidExceedsEscrow
            ),
            Side::Sell => require!(qty <= order.escrowed, AuctionError::AskExceedsEscrow),
        }
        order.revealed = true;
        order.limit = limit;
        order.qty = qty;
        let side = order.side;

        let rails = ctx.remaining_accounts;
        require!(rails.len() == 3, AuctionError::RailsAccounts);
        require_keys_eq!(
            rails[0].key(),
            session.rails_session,
            AuctionError::RailsAccounts
        );
        require_keys_eq!(
            rails[1].key(),
            rails_escrow(&session.rails_session, &owner),
            AuctionError::RailsAccounts
        );
        let operator_bump = ctx.accounts.venue.operator_bump;
        rails_call(
            &ctx.accounts.rails_program,
            rails,
            payment_rails::instruction::MarkRevealed {}.data(),
            &ctx.accounts.operator.key(),
            operator_bump,
        )?;
        emit!(OrderRevealed {
            session: session_key,
            owner,
            side,
            limit,
            qty,
        });
        Ok(())
    }

    /// AE-P5 + AE-P6 (margin): compute the uniform clearing price and each
    /// order's allocation. Permissionless once reveals close. A session
    /// with too few reveals, no cross at or above the reserve floor, or a
    /// suspended mark fails (rails `fail_session`; all escrow refundable).
    /// Remaining accounts: rails session, operator.
    pub fn clear<'info>(ctx: Context<'info, Clear<'info>>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let session_key = ctx.accounts.session.key();
        let session = &mut ctx.accounts.session;
        require!(
            now >= session.params.reveal_end,
            AuctionError::OutsideWindow
        );
        require!(
            now < session.params.settle_deadline,
            AuctionError::OutsideWindow
        );
        require!(
            matches!(session.phase, Phase::Commit | Phase::Reveal),
            AuctionError::WrongPhase
        );
        let outcome = compute_clear(session, &ctx.accounts.feed, now)?;
        match outcome {
            Ok(c) => {
                session.clear_price = c.price;
                session.cleared_volume = c.volume;
                session.reserve_price = c.reserve;
                session.reference_mark = c.reference;
                for (o, a) in session.orders.iter_mut().zip(c.allocations.iter()) {
                    o.alloc = *a;
                    o.alloc_left = *a;
                }
                emit!(Cleared {
                    session: session_key,
                    price: c.price,
                    volume: c.volume,
                    reserve: c.reserve,
                    reference: c.reference,
                });
                set_phase(session_key, session, Phase::Allocating, now);
                Ok(())
            }
            Err(reason) => {
                emit!(SessionFailed {
                    session: session_key,
                    reason,
                });
                set_phase(session_key, session, Phase::Failed, now);
                let rails = ctx.remaining_accounts;
                require!(rails.len() == 2, AuctionError::RailsAccounts);
                require_keys_eq!(
                    rails[0].key(),
                    session.rails_session,
                    AuctionError::RailsAccounts
                );
                rails_call(
                    &ctx.accounts.rails_program,
                    rails,
                    payment_rails::instruction::FailSession {}.data(),
                    &ctx.accounts.operator.key(),
                    ctx.accounts.venue.operator_bump,
                )
            }
        }
    }

    /// AE-P6: pair allocated buyers and sellers into at most `max_fills`
    /// new fills, in order-book order (deterministic, resumable). Moves to
    /// IssuerRofr (ROFR configured) or Settling when every allocation is
    /// paired. Permissionless.
    pub fn allocate(ctx: Context<Crank>, max_fills: u8) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let session_key = ctx.accounts.session.key();
        let session = &mut ctx.accounts.session;
        require!(session.phase == Phase::Allocating, AuctionError::WrongPhase);
        let mut made = 0u8;
        while made < max_fills {
            let b = session
                .orders
                .iter()
                .position(|o| o.side == Side::Buy && o.alloc_left > 0);
            let s = session
                .orders
                .iter()
                .position(|o| o.side == Side::Sell && o.alloc_left > 0);
            let (Some(b), Some(s)) = (b, s) else { break };
            require!(session.fills.len() < MAX_FILLS, AuctionError::BookFull);
            let qty = session.orders[b]
                .alloc_left
                .min(session.orders[s].alloc_left);
            session.orders[b].alloc_left -= qty;
            session.orders[s].alloc_left -= qty;
            let fill = Fill {
                buyer: session.orders[b].owner,
                seller: session.orders[s].owner,
                qty,
                rofr: false,
                settled: false,
            };
            emit!(FillAllocated {
                session: session_key,
                fill_index: session.fills.len() as u32,
                buyer: fill.buyer,
                seller: fill.seller,
                qty,
            });
            session.fills.push(fill);
            made += 1;
        }
        let done = session.orders.iter().all(|o| o.alloc_left == 0);
        if done {
            let next = if session.params.rofr_authority == Pubkey::default() {
                Phase::Settling
            } else {
                Phase::IssuerRofr
            };
            set_phase(session_key, session, next, now);
        }
        Ok(())
    }

    /// AE-P7: the issuer takes `qty` of fill `fill_index` at the clearing
    /// price, paid from its own account and settled at once (rails
    /// `settle_rofr_fill`). The buyer's allocation shrinks by `qty`; their
    /// unused cash is refundable after `finish`. Remaining accounts: the
    /// rails `SettleRofrFill` list for the new fill index.
    pub fn exercise_rofr<'info>(
        ctx: Context<'info, ExerciseRofr<'info>>,
        fill_index: u32,
        qty: u64,
    ) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let session_key = ctx.accounts.session.key();
        let issuer = ctx.accounts.issuer.key();
        let session = &mut ctx.accounts.session;
        require!(session.phase == Phase::IssuerRofr, AuctionError::WrongPhase);
        require!(now < session.rofr_end(), AuctionError::OutsideWindow);
        require_keys_eq!(
            issuer,
            session.params.rofr_authority,
            AuctionError::NotRofrAuthority
        );
        require!(qty > 0, AuctionError::ZeroAmount);
        require!(session.fills.len() < MAX_FILLS, AuctionError::BookFull);
        let i = fill_index as usize;
        let fill = session.fills.get_mut(i).ok_or(AuctionError::NoFill)?;
        require!(!fill.rofr && !fill.settled, AuctionError::NoFill);
        require!(qty <= fill.qty, AuctionError::RofrExceedsFill);
        fill.qty -= qty;
        let seller = fill.seller;
        let new_index = session.fills.len() as u32;
        session.fills.push(Fill {
            buyer: issuer,
            seller,
            qty,
            rofr: true,
            settled: true,
        });
        session.rofr_qty = session.rofr_qty.saturating_add(qty);

        let rails = ctx.remaining_accounts;
        require!(rails.len() == 23, AuctionError::RailsAccounts);
        require_keys_eq!(
            rails[1].key(),
            session.rails_session,
            AuctionError::RailsAccounts
        );
        require_keys_eq!(rails[3].key(), issuer, AuctionError::RailsAccounts);
        require_keys_eq!(
            rails[5].key(),
            rails_escrow(&session.rails_session, &seller),
            AuctionError::RailsAccounts
        );
        let rent = Rent::get()?.minimum_balance(8 + payment_rails::FillReceipt::INIT_SPACE);
        top_up(
            &ctx.accounts.issuer,
            &ctx.accounts.operator,
            &ctx.accounts.system_program,
            rent,
        )?;
        let data = payment_rails::instruction::SettleRofrFill {
            fill_index: new_index,
            qty,
            price: session.clear_price,
            disclosure_hash: session.params.disclosure_hash,
        }
        .data();
        rails_call(
            &ctx.accounts.rails_program,
            rails,
            data,
            &ctx.accounts.operator.key(),
            ctx.accounts.venue.operator_bump,
        )?;
        emit!(RofrExercised {
            session: session_key,
            fill_index,
            rofr_fill_index: new_index,
            qty,
            price: session.clear_price,
            at: now,
        });
        Ok(())
    }

    /// AE-P7: end the ROFR window — the issuer declines (or is done), or
    /// anyone once `rofr_end` has passed.
    pub fn close_rofr(ctx: Context<CloseRofr>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let session_key = ctx.accounts.session.key();
        let caller = ctx.accounts.caller.key();
        let session = &mut ctx.accounts.session;
        require!(session.phase == Phase::IssuerRofr, AuctionError::WrongPhase);
        require!(
            caller == session.params.rofr_authority || now >= session.rofr_end(),
            AuctionError::NotRofrAuthority
        );
        emit!(RofrClosed {
            session: session_key,
            exercised_qty: session.rofr_qty,
            at: now,
        });
        set_phase(session_key, session, Phase::Settling, now);
        Ok(())
    }

    /// AE-P8: settle one fill through rails `settle_fill` (atomic DvP +
    /// credit-gate `settle_check`). Permissionless; the caller funds the
    /// fill receipt's rent. Remaining accounts: the rails `SettleFill` list.
    pub fn settle<'info>(ctx: Context<'info, Settle<'info>>, fill_index: u32) -> Result<()> {
        let session_key = ctx.accounts.session.key();
        let session = &mut ctx.accounts.session;
        require!(session.phase == Phase::Settling, AuctionError::WrongPhase);
        let price = session.clear_price;
        let disclosure_hash = session.params.disclosure_hash;
        let rails_session = session.rails_session;
        let fill = session
            .fills
            .get_mut(fill_index as usize)
            .ok_or(AuctionError::NoFill)?;
        require!(!fill.settled, AuctionError::FillSettled);
        fill.settled = true;
        let f = *fill;
        session.fills_settled = session.fills_settled.saturating_add(1);
        if f.qty == 0 {
            // Taken in full by the issuer's ROFR: nothing left to move.
            return Ok(());
        }
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 28, AuctionError::RailsAccounts);
        require_keys_eq!(rails[1].key(), rails_session, AuctionError::RailsAccounts);
        require_keys_eq!(
            rails[4].key(),
            rails_escrow(&rails_session, &f.buyer),
            AuctionError::RailsAccounts
        );
        require_keys_eq!(
            rails[6].key(),
            rails_escrow(&rails_session, &f.seller),
            AuctionError::RailsAccounts
        );
        let rent = Rent::get()?.minimum_balance(8 + payment_rails::FillReceipt::INIT_SPACE);
        top_up(
            &ctx.accounts.payer,
            &ctx.accounts.operator,
            &ctx.accounts.system_program,
            rent,
        )?;
        let data = payment_rails::instruction::SettleFill {
            fill_index,
            qty: f.qty,
            price,
            disclosure_hash,
        }
        .data();
        rails_call(
            &ctx.accounts.rails_program,
            rails,
            data,
            &ctx.accounts.operator.key(),
            ctx.accounts.venue.operator_bump,
        )?;
        emit!(FillSettled {
            session: session_key,
            fill_index,
            buyer: f.buyer,
            seller: f.seller,
            qty: f.qty,
            price,
        });
        Ok(())
    }

    /// AE-P9: every fill settled → rails `finish_session` (unfilled escrow
    /// refundable at once), then the clearing print into mark-engine.
    /// Permissionless; the caller pays the print record's rent.
    /// Remaining accounts: rails session, operator.
    pub fn finish<'info>(ctx: Context<'info, Finish<'info>>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let session_key = ctx.accounts.session.key();
        let session = &mut ctx.accounts.session;
        require!(session.phase == Phase::Settling, AuctionError::WrongPhase);
        require!(
            session.fills.iter().all(|f| f.settled),
            AuctionError::FillsPending
        );
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 2, AuctionError::RailsAccounts);
        require_keys_eq!(
            rails[0].key(),
            session.rails_session,
            AuctionError::RailsAccounts
        );
        rails_call(
            &ctx.accounts.rails_program,
            rails,
            payment_rails::instruction::FinishSession {}.data(),
            &ctx.accounts.operator.key(),
            ctx.accounts.venue.operator_bump,
        )?;

        let mut wallets: Vec<Pubkey> = Vec::with_capacity(session.fills.len() * 2);
        for f in session.fills.iter().filter(|f| f.qty > 0) {
            for w in [f.buyer, f.seller] {
                if !wallets.contains(&w) {
                    wallets.push(w);
                }
            }
        }
        let n_investor_ids = wallets.len() as u16;
        let policy = ctx.accounts.feed.params.print_policy;
        let print = PrintInput {
            session: session_key,
            price: session.clear_price,
            volume: session.cleared_volume,
            n_investor_ids,
            forced_sale: session.params.forced_sale,
        };
        let seeds: &[&[u8]] = &[PRINT_SEED, &[ctx.accounts.venue.print_bump]];
        mark_engine::cpi::record_print(
            CpiContext::new_with_signer(
                mark_engine::ID,
                mark_engine::cpi::accounts::RecordPrint {
                    feed: ctx.accounts.feed.to_account_info(),
                    record: ctx.accounts.print_record.to_account_info(),
                    print_source: ctx.accounts.print_authority.to_account_info(),
                    payer: ctx.accounts.payer.to_account_info(),
                    system_program: ctx.accounts.system_program.to_account_info(),
                },
                &[seeds],
            ),
            print,
        )?;
        emit!(ClearingPrint {
            session: session_key,
            asset_mint: session.asset_mint,
            price: session.clear_price,
            volume: session.cleared_volume,
            n_investor_ids,
            forced_sale: session.params.forced_sale,
            print_policy_class: policy,
        });
        set_phase(session_key, session, Phase::Printed, now);
        Ok(())
    }

    /// AE-P10: past `settle_deadline` (or past `reveal_end` while still in
    /// Commit/Reveal and uncleared for the whole settle window), the session
    /// fails; the rails then refund every escrow permissionlessly.
    /// Remaining accounts: rails session, operator.
    pub fn expire<'info>(ctx: Context<'info, Clear<'info>>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let session_key = ctx.accounts.session.key();
        let session = &mut ctx.accounts.session;
        require!(
            !matches!(session.phase, Phase::Printed | Phase::Failed),
            AuctionError::WrongPhase
        );
        require!(
            now >= session.params.settle_deadline,
            AuctionError::OutsideWindow
        );
        emit!(SessionFailed {
            session: session_key,
            reason: FailReason::Deadline,
        });
        set_phase(session_key, session, Phase::Failed, now);
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 2, AuctionError::RailsAccounts);
        require_keys_eq!(
            rails[0].key(),
            session.rails_session,
            AuctionError::RailsAccounts
        );
        // The rails session may already be closed for settlement; only an
        // open one is failed (the rails refund either way).
        let rails_state =
            payment_rails::CashSession::try_deserialize(&mut &rails[0].try_borrow_data()?[..])?;
        if rails_state.status == payment_rails::SessionStatus::Open {
            rails_call(
                &ctx.accounts.rails_program,
                rails,
                payment_rails::instruction::FailSession {}.data(),
                &ctx.accounts.operator.key(),
                ctx.accounts.venue.operator_bump,
            )?;
        }
        Ok(())
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

fn commit<'info>(
    ctx: Context<'info, Commit<'info>>,
    side: Side,
    commitment_hash: [u8; 32],
    escrowed: u64,
    rails_data: Vec<u8>,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let owner = ctx.accounts.owner.key();
    let session_key = ctx.accounts.session.key();
    let session = &mut ctx.accounts.session;
    require!(session.phase == Phase::Commit, AuctionError::WrongPhase);
    require!(now < session.params.commit_end, AuctionError::OutsideWindow);
    require!(escrowed > 0, AuctionError::ZeroAmount);
    require!(session.orders.len() < MAX_ORDERS, AuctionError::BookFull);
    require!(
        session.orders.iter().all(|o| o.owner != owner),
        AuctionError::DuplicateOrder
    );
    let rails = ctx.remaining_accounts;
    require_keys_eq!(
        rails[1].key(),
        session.rails_session,
        AuctionError::RailsAccounts
    );
    session.orders.push(Order {
        owner,
        side,
        commitment: commitment_hash,
        escrowed,
        revealed: false,
        limit: 0,
        qty: 0,
        alloc: 0,
        alloc_left: 0,
    });
    // The owner signs the outer transaction; the rails deposit runs under
    // that signature (no engine signer needed).
    let metas = rails
        .iter()
        .map(|a| AccountMeta {
            pubkey: a.key(),
            is_signer: a.is_signer,
            is_writable: a.is_writable,
        })
        .collect();
    let mut infos = rails.to_vec();
    infos.push(ctx.accounts.rails_program.to_account_info());
    anchor_lang::solana_program::program::invoke(
        &Instruction {
            program_id: payment_rails::ID,
            accounts: metas,
            data: rails_data,
        },
        &infos,
    )?;
    emit!(OrderCommitted {
        session: session_key,
        owner,
        side,
        commitment: commitment_hash,
        escrowed,
    });
    Ok(())
}

/// Forward `rails` to payment-rails with `data`, the operator PDA signing.
fn rails_call<'info>(
    rails_program: &AccountInfo<'info>,
    rails: &[AccountInfo<'info>],
    data: Vec<u8>,
    operator: &Pubkey,
    operator_bump: u8,
) -> Result<()> {
    require_keys_eq!(
        rails_program.key(),
        payment_rails::ID,
        AuctionError::RailsAccounts
    );
    let metas = rails
        .iter()
        .map(|a| AccountMeta {
            pubkey: a.key(),
            is_signer: a.is_signer || a.key() == *operator,
            is_writable: a.is_writable,
        })
        .collect();
    let mut infos = rails.to_vec();
    infos.push(rails_program.clone());
    invoke_signed(
        &Instruction {
            program_id: payment_rails::ID,
            accounts: metas,
            data,
        },
        &infos,
        &[&[OPERATOR_SEED, &[operator_bump]]],
    )
    .map_err(Into::into)
}

/// Move `lamports` from `from` into the operator PDA (its rent float).
fn top_up<'info>(
    from: &Signer<'info>,
    operator: &UncheckedAccount<'info>,
    system: &Program<'info, System>,
    lamports: u64,
) -> Result<()> {
    system_program::transfer(
        CpiContext::new(
            system.key(),
            system_program::Transfer {
                from: from.to_account_info(),
                to: operator.to_account_info(),
            },
        ),
        lamports,
    )
}

/// The rails escrow PDA of `owner` in `rails_session`.
pub fn rails_escrow(rails_session: &Pubkey, owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            payment_rails::Escrow::SEED,
            rails_session.as_ref(),
            owner.as_ref(),
        ],
        &payment_rails::ID,
    )
    .0
}

/// Cash for `qty` at `price`, rounded up exactly as the rails charge it.
pub fn gross(qty: u64, price: u64, decimals: u8) -> Result<u64> {
    let scale = 10u128.pow(u32::from(decimals));
    let v = credit_math::checked_ceil_div(
        u128::from(qty)
            .checked_mul(u128::from(price))
            .ok_or(AuctionError::MathOverflow)?,
        scale,
    )
    .map_err(|_| AuctionError::MathOverflow)?;
    u64::try_from(v).map_err(|_| error!(AuctionError::MathOverflow))
}

fn set_phase(key: Pubkey, session: &mut Session, to: Phase, at: i64) {
    let from = session.phase;
    session.phase = to;
    emit!(PhaseChanged {
        session: key,
        from: Some(from),
        to,
        at,
    });
}

struct ClearOk {
    price: u64,
    volume: u64,
    reserve: u64,
    reference: u64,
    allocations: Vec<u64>,
}

/// AE-P5/P6: the clearing outcome, or why the session fails.
fn compute_clear(
    session: &Session,
    feed: &Feed,
    now: i64,
) -> Result<core::result::Result<ClearOk, FailReason>> {
    let revealed = session.orders.iter().filter(|o| o.revealed).count();
    if revealed < usize::from(session.params.min_reveals) {
        return Ok(Err(FailReason::TooFewReveals));
    }
    let (reserve, reference) = if session.params.forced_sale {
        // AE-P11: reserve relaxed; reference still the protective mark.
        let reference = if feed.nav_pico > 0 {
            feed.pico_to_price(feed.mark_protective(now)?)?
        } else {
            0
        };
        (0u64, reference)
    } else {
        if feed.state_at(now) == MarkState::Suspended || feed.nav_pico == 0 {
            return Ok(Err(FailReason::MarkUnavailable));
        }
        let eff = feed.nav_effective(now)?;
        let floor = mul_div(
            eff,
            u128::from(BPS - session.params.max_discount_bps),
            u128::from(BPS),
        )
        .map_err(|_| AuctionError::MathOverflow)?;
        // Reserve rounds up (a seller's floor is never undercut by rounding).
        let reserve = feed.pico_to_price(floor)?;
        let reserve = if feed.price_to_pico(reserve)? < floor {
            reserve + 1
        } else {
            reserve
        };
        (reserve, feed.pico_to_price(feed.mark_protective(now)?)?)
    };
    let orders: Vec<MathOrder> = session
        .orders
        .iter()
        .enumerate()
        .filter(|(_, o)| o.revealed)
        .map(|(i, o)| MathOrder {
            bidder_id: i as u64,
            limit_price: u128::from(o.limit),
            qty: u128::from(o.qty),
            is_buy: o.side == Side::Buy,
        })
        .collect();
    let Some(c) = find_uniform_clear_price(&orders, u128::from(reserve), u128::from(reference))
        .map_err(|_| AuctionError::MathOverflow)?
    else {
        return Ok(Err(FailReason::NoCross));
    };
    let price = u64::try_from(c.clear_price).map_err(|_| AuctionError::MathOverflow)?;
    let in_buy = |o: &MathOrder| o.is_buy && o.limit_price >= c.clear_price;
    let in_sell = |o: &MathOrder| !o.is_buy && o.limit_price <= c.clear_price;
    let mut allocations = vec![0u64; session.orders.len()];
    for in_side in [&in_buy as &dyn Fn(&MathOrder) -> bool, &in_sell] {
        let side: Vec<(u64, u128)> = orders
            .iter()
            .filter(|o| in_side(o))
            .map(|o| (o.bidder_id, o.qty))
            .collect();
        // Full fills on the short side; pro-rata at the margin on the long
        // side (sums exactly to the executed volume).
        for (id, q) in
            allocate_pro_rata(c.executed_qty, &side).map_err(|_| AuctionError::MathOverflow)?
        {
            allocations[id as usize] = u64::try_from(q).map_err(|_| AuctionError::MathOverflow)?;
        }
    }
    Ok(Ok(ClearOk {
        price,
        volume: u64::try_from(c.executed_qty).map_err(|_| AuctionError::MathOverflow)?,
        reserve,
        reference,
        allocations,
    }))
}

// ── state ───────────────────────────────────────────────────────────────

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum Side {
    Buy,
    Sell,
}

/// AE-P2 phases (06 §2).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum Phase {
    Commit,
    Reveal,
    Allocating,
    IssuerRofr,
    Settling,
    Printed,
    Failed,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum FailReason {
    TooFewReveals,
    NoCross,
    MarkUnavailable,
    Deadline,
}

/// AE-P1: everything snapshotted at open; immutable afterwards.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct SessionParams {
    pub commit_end: i64,
    pub reveal_end: i64,
    /// ROFR window length after `reveal_end` (0 with no ROFR authority).
    pub rofr_window: i64,
    /// Hard deadline: settlement closes, every escrow refundable.
    pub settle_deadline: i64,
    /// Forfeitable bond per order (micro-USDC).
    pub bond: u64,
    /// Per-investor cash cap (micro-USDC).
    pub max_cash: u64,
    /// Fewer revealed orders than this ⇒ Failed (G13).
    pub min_reveals: u8,
    /// Reserve floor = NAV_effective × (1 − max_discount_bps).
    pub max_discount_bps: u64,
    /// AE-P11: opened by the facility; no reserve; flagged print.
    pub forced_sale: bool,
    /// Issuer ROFR authority; `Pubkey::default()` disables ROFR.
    pub rofr_authority: Pubkey,
    /// Hash of the session disclosure, carried on every transfer record.
    pub disclosure_hash: [u8; 32],
}

impl SessionParams {
    pub fn validate(&self, now: i64) -> Result<()> {
        let rofr = self.rofr_authority != Pubkey::default();
        require!(
            now < self.commit_end
                && self.commit_end < self.reveal_end
                && self.reveal_end < self.settle_deadline
                && self.rofr_window >= 0
                && (rofr || self.rofr_window == 0)
                && self.reveal_end + self.rofr_window < self.settle_deadline,
            AuctionError::InvalidSchedule
        );
        require!(self.bond > 0 && self.max_cash > 0, AuctionError::ZeroAmount);
        require!(self.min_reveals >= 2, AuctionError::InvalidSchedule);
        require!(
            self.max_discount_bps <= MAX_DISCOUNT_BPS,
            AuctionError::DiscountAboveCeiling
        );
        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct Order {
    pub owner: Pubkey,
    pub side: Side,
    pub commitment: [u8; 32],
    /// Cash (buy) or asset (sell) escrowed in the rails at commit.
    pub escrowed: u64,
    pub revealed: bool,
    pub limit: u64,
    pub qty: u64,
    /// Allocation at the clearing price.
    pub alloc: u64,
    /// Allocation not yet paired into a fill (AE-P6 cursor).
    pub alloc_left: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct Fill {
    pub buyer: Pubkey,
    pub seller: Pubkey,
    pub qty: u64,
    /// Issuer ROFR fill (settled at exercise).
    pub rofr: bool,
    pub settled: bool,
}

#[account]
#[derive(InitSpace)]
pub struct Venue {
    pub authority: Pubkey,
    pub forced_sale_opener: Pubkey,
    pub sessions: u64,
    pub bump: u8,
    pub operator_bump: u8,
    pub print_bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Session {
    pub venue: Pubkey,
    pub session_id: u64,
    pub asset_mint: Pubkey,
    pub asset_decimals: u8,
    /// mark-engine feed of the asset.
    pub feed: Pubkey,
    /// payment-rails `CashSession` (operator = this program's PDA).
    pub rails_session: Pubkey,
    pub params: SessionParams,
    pub phase: Phase,
    pub reserve_price: u64,
    pub reference_mark: u64,
    pub clear_price: u64,
    pub cleared_volume: u64,
    pub rofr_qty: u64,
    pub fills_settled: u32,
    #[max_len(MAX_ORDERS)]
    pub orders: Vec<Order>,
    #[max_len(MAX_FILLS)]
    pub fills: Vec<Fill>,
    pub bump: u8,
}

impl Session {
    pub fn rofr_end(&self) -> i64 {
        self.params.reveal_end + self.params.rofr_window
    }
}

// ── accounts ────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct InitializeVenue<'info> {
    #[account(init, payer = authority, space = 8 + Venue::INIT_SPACE, seeds = [VENUE_SEED], bump)]
    pub venue: Account<'info, Venue>,
    /// CHECK: the operator PDA (system account, rent float only).
    #[account(mut, seeds = [OPERATOR_SEED], bump)]
    pub operator: UncheckedAccount<'info>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct VenueAdmin<'info> {
    #[account(mut, seeds = [VENUE_SEED], bump = venue.bump, has_one = authority @ AuctionError::NotVenueAuthority)]
    pub venue: Account<'info, Venue>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(session_id: u64)]
pub struct OpenSession<'info> {
    #[account(mut, seeds = [VENUE_SEED], bump = venue.bump)]
    pub venue: Box<Account<'info, Venue>>,
    #[account(
        init,
        payer = opener,
        space = 8 + Session::INIT_SPACE,
        seeds = [SESSION_SEED, venue.key().as_ref(), &session_id.to_le_bytes()],
        bump
    )]
    pub session: Box<Account<'info, Session>>,
    /// The asset's mark-engine feed (reserve + reference + print target).
    pub feed: Box<Account<'info, Feed>>,
    /// CHECK: the operator PDA; signs the rails CPI.
    #[account(mut, seeds = [OPERATOR_SEED], bump = venue.operator_bump)]
    pub operator: UncheckedAccount<'info>,
    /// Venue authority, or the forced-sale opener for a forced sale.
    #[account(mut)]
    pub opener: Signer<'info>,
    /// CHECK: payment-rails (address-checked in `rails_call`).
    pub rails_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Commit<'info> {
    #[account(mut, seeds = [SESSION_SEED, session.venue.as_ref(), &session.session_id.to_le_bytes()], bump = session.bump)]
    pub session: Box<Account<'info, Session>>,
    #[account(mut)]
    pub owner: Signer<'info>,
    /// CHECK: payment-rails, address-pinned.
    #[account(address = payment_rails::ID @ AuctionError::RailsAccounts)]
    pub rails_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Reveal<'info> {
    #[account(seeds = [VENUE_SEED], bump = venue.bump)]
    pub venue: Box<Account<'info, Venue>>,
    #[account(mut, seeds = [SESSION_SEED, venue.key().as_ref(), &session.session_id.to_le_bytes()], bump = session.bump)]
    pub session: Box<Account<'info, Session>>,
    pub owner: Signer<'info>,
    /// CHECK: the operator PDA; signs the rails CPI.
    #[account(seeds = [OPERATOR_SEED], bump = venue.operator_bump)]
    pub operator: UncheckedAccount<'info>,
    /// CHECK: payment-rails (address-checked in `rails_call`).
    pub rails_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Clear<'info> {
    #[account(seeds = [VENUE_SEED], bump = venue.bump)]
    pub venue: Box<Account<'info, Venue>>,
    #[account(mut, seeds = [SESSION_SEED, venue.key().as_ref(), &session.session_id.to_le_bytes()], bump = session.bump)]
    pub session: Box<Account<'info, Session>>,
    #[account(address = session.feed @ AuctionError::FeedMismatch)]
    pub feed: Box<Account<'info, Feed>>,
    /// CHECK: the operator PDA; signs the rails CPI.
    #[account(seeds = [OPERATOR_SEED], bump = venue.operator_bump)]
    pub operator: UncheckedAccount<'info>,
    /// CHECK: payment-rails (address-checked in `rails_call`).
    pub rails_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Crank<'info> {
    #[account(mut, seeds = [SESSION_SEED, session.venue.as_ref(), &session.session_id.to_le_bytes()], bump = session.bump)]
    pub session: Box<Account<'info, Session>>,
}

#[derive(Accounts)]
pub struct ExerciseRofr<'info> {
    #[account(seeds = [VENUE_SEED], bump = venue.bump)]
    pub venue: Box<Account<'info, Venue>>,
    #[account(mut, seeds = [SESSION_SEED, venue.key().as_ref(), &session.session_id.to_le_bytes()], bump = session.bump)]
    pub session: Box<Account<'info, Session>>,
    /// The session's ROFR authority; pays cash and the receipt rent.
    #[account(mut)]
    pub issuer: Signer<'info>,
    /// CHECK: the operator PDA; signs the rails CPI.
    #[account(mut, seeds = [OPERATOR_SEED], bump = venue.operator_bump)]
    pub operator: UncheckedAccount<'info>,
    /// CHECK: payment-rails (address-checked in `rails_call`).
    pub rails_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CloseRofr<'info> {
    #[account(mut, seeds = [SESSION_SEED, session.venue.as_ref(), &session.session_id.to_le_bytes()], bump = session.bump)]
    pub session: Box<Account<'info, Session>>,
    pub caller: Signer<'info>,
}

#[derive(Accounts)]
pub struct Settle<'info> {
    #[account(seeds = [VENUE_SEED], bump = venue.bump)]
    pub venue: Box<Account<'info, Venue>>,
    #[account(mut, seeds = [SESSION_SEED, venue.key().as_ref(), &session.session_id.to_le_bytes()], bump = session.bump)]
    pub session: Box<Account<'info, Session>>,
    /// CHECK: the operator PDA; signs the rails CPI.
    #[account(mut, seeds = [OPERATOR_SEED], bump = venue.operator_bump)]
    pub operator: UncheckedAccount<'info>,
    /// Crank: funds the rails fill receipt.
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: payment-rails (address-checked in `rails_call`).
    pub rails_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Finish<'info> {
    #[account(seeds = [VENUE_SEED], bump = venue.bump)]
    pub venue: Box<Account<'info, Venue>>,
    #[account(mut, seeds = [SESSION_SEED, venue.key().as_ref(), &session.session_id.to_le_bytes()], bump = session.bump)]
    pub session: Box<Account<'info, Session>>,
    #[account(mut, address = session.feed @ AuctionError::FeedMismatch)]
    pub feed: Box<Account<'info, Feed>>,
    /// CHECK: created by mark-engine (`["print", feed, session]`).
    #[account(mut)]
    pub print_record: UncheckedAccount<'info>,
    /// CHECK: the print PDA; signs `record_print`.
    #[account(seeds = [PRINT_SEED], bump = venue.print_bump)]
    pub print_authority: UncheckedAccount<'info>,
    /// CHECK: the operator PDA; signs the rails CPI.
    #[account(seeds = [OPERATOR_SEED], bump = venue.operator_bump)]
    pub operator: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: payment-rails (address-checked in `rails_call`).
    pub rails_program: UncheckedAccount<'info>,
    /// CHECK: mark-engine, address-pinned.
    #[account(address = mark_engine::ID @ AuctionError::FeedMismatch)]
    pub mark_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

// ── events ──────────────────────────────────────────────────────────────

#[event]
pub struct VenueConfigured {
    pub authority: Pubkey,
    pub forced_sale_opener: Pubkey,
}

#[event]
pub struct PhaseChanged {
    pub session: Pubkey,
    pub from: Option<Phase>,
    pub to: Phase,
    pub at: i64,
}

#[event]
pub struct OrderCommitted {
    pub session: Pubkey,
    pub owner: Pubkey,
    pub side: Side,
    pub commitment: [u8; 32],
    pub escrowed: u64,
}

#[event]
pub struct OrderRevealed {
    pub session: Pubkey,
    pub owner: Pubkey,
    pub side: Side,
    pub limit: u64,
    pub qty: u64,
}

#[event]
pub struct Cleared {
    pub session: Pubkey,
    pub price: u64,
    pub volume: u64,
    pub reserve: u64,
    pub reference: u64,
}

#[event]
pub struct SessionFailed {
    pub session: Pubkey,
    pub reason: FailReason,
}

#[event]
pub struct FillAllocated {
    pub session: Pubkey,
    pub fill_index: u32,
    pub buyer: Pubkey,
    pub seller: Pubkey,
    pub qty: u64,
}

#[event]
pub struct RofrExercised {
    pub session: Pubkey,
    pub fill_index: u32,
    pub rofr_fill_index: u32,
    pub qty: u64,
    pub price: u64,
    pub at: i64,
}

#[event]
pub struct RofrClosed {
    pub session: Pubkey,
    pub exercised_qty: u64,
    pub at: i64,
}

#[event]
pub struct FillSettled {
    pub session: Pubkey,
    pub fill_index: u32,
    pub buyer: Pubkey,
    pub seller: Pubkey,
    pub qty: u64,
    pub price: u64,
}

/// AE-P9: the session's aggregate print (mark-engine stores its own copy,
/// disclosed per the market's print policy).
#[event]
pub struct ClearingPrint {
    pub session: Pubkey,
    pub asset_mint: Pubkey,
    pub price: u64,
    pub volume: u64,
    pub n_investor_ids: u16,
    pub forced_sale: bool,
    pub print_policy_class: PrintPolicy,
}

// ── errors (append-only) ────────────────────────────────────────────────

#[error_code]
pub enum AuctionError {
    #[msg("signer is not the venue authority")]
    NotVenueAuthority,
    #[msg("only the forced-sale opener may open a forced-sale session")]
    NotForcedSaleOpener,
    #[msg("session schedule or minimums invalid")]
    InvalidSchedule,
    #[msg("reserve discount above the hard ceiling")]
    DiscountAboveCeiling,
    #[msg("amount must be positive")]
    ZeroAmount,
    #[msg("payment-rails accounts do not match the session")]
    RailsAccounts,
    #[msg("feed does not belong to the session's asset")]
    FeedMismatch,
    #[msg("asset mint does not match the feed")]
    AssetMismatch,
    #[msg("instruction outside its time window")]
    OutsideWindow,
    #[msg("instruction not allowed in this phase")]
    WrongPhase,
    #[msg("order book or fill list is full")]
    BookFull,
    #[msg("this wallet already has an order in the session")]
    DuplicateOrder,
    #[msg("no order for this wallet")]
    NoOrder,
    #[msg("order already revealed")]
    AlreadyRevealed,
    #[msg("preimage does not match the commitment")]
    CommitmentMismatch,
    #[msg("revealed bid costs more than the escrowed cash")]
    BidExceedsEscrow,
    #[msg("revealed ask exceeds the escrowed asset")]
    AskExceedsEscrow,
    #[msg("signer is not the session's ROFR authority")]
    NotRofrAuthority,
    #[msg("no such fill (or not eligible)")]
    NoFill,
    #[msg("ROFR quantity exceeds the fill")]
    RofrExceedsFill,
    #[msg("fill already settled")]
    FillSettled,
    #[msg("fills still pending settlement")]
    FillsPending,
    #[msg("arithmetic overflow")]
    MathOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commitment_binds_every_field() {
        let o = Pubkey::new_unique();
        let n = [3u8; 32];
        let c = commitment(Side::Buy, 10, 20, &n, &o);
        assert_ne!(c, commitment(Side::Sell, 10, 20, &n, &o));
        assert_ne!(c, commitment(Side::Buy, 11, 20, &n, &o));
        assert_ne!(c, commitment(Side::Buy, 10, 21, &n, &o));
        assert_ne!(c, commitment(Side::Buy, 10, 20, &[4; 32], &o));
        assert_ne!(c, commitment(Side::Buy, 10, 20, &n, &Pubkey::new_unique()));
        assert_eq!(c, commitment(Side::Buy, 10, 20, &n, &o));
    }

    #[test]
    fn gross_rounds_up_like_the_rails() {
        assert_eq!(gross(1_000_000, 950_000, 6).unwrap(), 950_000);
        assert_eq!(gross(1, 950_000, 6).unwrap(), 1, "ceil");
        assert_eq!(gross(3, 333_333, 6).unwrap(), 1);
    }

    #[test]
    fn params_bounds() {
        let p = SessionParams {
            commit_end: 10,
            reveal_end: 20,
            rofr_window: 5,
            settle_deadline: 40,
            bond: 1,
            max_cash: 1,
            min_reveals: 2,
            max_discount_bps: 1_000,
            forced_sale: false,
            rofr_authority: Pubkey::new_unique(),
            disclosure_hash: [0; 32],
        };
        assert!(p.validate(0).is_ok());
        assert!(p.validate(10).is_err(), "commit window already over");
        let mut q = p;
        q.max_discount_bps = MAX_DISCOUNT_BPS + 1;
        assert!(q.validate(0).is_err());
        let mut q = p;
        q.rofr_authority = Pubkey::default();
        assert!(q.validate(0).is_err(), "ROFR window without an authority");
        let mut q = p;
        q.rofr_window = 20;
        assert!(q.validate(0).is_err(), "ROFR must end before the deadline");
        let mut q = p;
        q.min_reveals = 1;
        assert!(q.validate(0).is_err());
    }
}
