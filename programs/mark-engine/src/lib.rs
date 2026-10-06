//! `mark-engine` — NAV feeds, mark states, auction prints, MARK_PROTECTIVE.
//!
//! SRS 08 §4.3 (ME-P1..P7) · doc 06 §3/§11 · #137.
//!
//! One [`Feed`] PDA per asset mint holds the latest published NAV and the
//! implied curve built from eligible auction prints. Every consumer
//! (auction-engine reserve floor, liquidity-facility underwriting and the
//! no-liquidation-on-stale-mark rule, tranche-engine MARK book) reads the
//! feed account and calls its pure methods — [`Feed::state_at`],
//! [`Feed::nav_effective`], [`Feed::mark_protective`],
//! [`Feed::allows_liquidation`] — so no consumer can compute a mark any
//! other way, and no CPI is needed to read one.
//!
//! - ME-P1 `publish_nav`: only a key in the feed's publisher set (the
//!   nav-publisher, which is also the SAS attester of the NAV attestation it
//!   signs off-chain) may write; `computed_ts` is monotone; `model_version`
//!   and `inputs_hash` are stored so every mark is reproducible (ME-1, AC-12).
//! - ME-P2 states Fresh → Warned → Stale by age, Suspended only by the
//!   authority; `refresh` (permissionless) events each transition.
//! - ME-P3 `NAV_effective = NAV × (1 − haircut(state))`, haircuts bounded by
//!   the hard ceilings below, which governance cannot exceed.
//! - ME-P4 `record_print`: only auction-engine's print authority may report
//!   a clearing print; eligibility (not forced-sale, min volume, min distinct
//!   investors, outlier band vs the official NAV) decides whether it enters
//!   the implied curve. Rejected prints are stored, flagged.
//! - ME-P5 `MARK_PROTECTIVE = min(NAV_effective, NAV_implied)`.
//! - ME-P6 print policy classes shape what the print event discloses.
//! - ME-P7 the feed's state maps onto the system breaker hierarchy; a
//!   Stale or Suspended mark forbids liquidation for every consumer.

#![deny(unsafe_code)]
#![allow(missing_docs)]
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use credit_math::{haircut_bps, mul_div};

declare_id!("79ypVjMMCDbmxYWaXTTgxz9jidjncoJAhHQC6e13x4LH");

/// Publisher keys per feed (rotation: add new → publish → remove old).
pub const MAX_PUBLISHERS: usize = 4;
/// Hard ceiling on the Warned haircut (ME-P3; governance cannot exceed).
pub const MAX_WARNED_HAIRCUT_BPS: u64 = 1_000;
/// Hard ceiling on the Stale/Suspended haircut.
pub const MAX_STALE_HAIRCUT_BPS: u64 = 5_000;
/// Hard ceiling on the outlier band of print eligibility (ME-P4).
pub const MAX_OUTLIER_BAND_BPS: u64 = 5_000;
/// Warn threshold floor: one hour (a sub-hour warn makes every mark Warned).
pub const MIN_WARN_SECONDS: i64 = 3_600;
/// Stale threshold ceiling: 60 days (a weekly mark must go stale eventually).
pub const MAX_STALE_SECONDS: i64 = 60 * 86_400;
/// NAV scale: NAVs are micro-USDC per base unit × 10¹², as
/// `credit_math::nav_gross_pico` computes them ("pico" below).
pub const NAV_SCALE: u128 = credit_math::nav::PICO;
/// The ceiling `haircut_bps` validates against: every haircut is ≤ 100%.
const BPS: u64 = 10_000;

#[program]
pub mod mark_engine {
    use super::*;

    /// Create the feed for `asset_mint`. The signer becomes the feed
    /// authority (the protocol Squads vault on devnet/mainnet).
    pub fn initialize_feed(ctx: Context<InitializeFeed>, params: FeedParams) -> Result<()> {
        params.validate()?;
        let decimals = mint_decimals(&ctx.accounts.asset_mint)?;
        let feed = &mut ctx.accounts.feed;
        feed.asset_mint = ctx.accounts.asset_mint.key();
        feed.asset_decimals = decimals;
        feed.authority = ctx.accounts.authority.key();
        feed.print_source = Pubkey::default();
        feed.publishers = [Pubkey::default(); MAX_PUBLISHERS];
        feed.params = params;
        feed.last_state = MarkState::Stale;
        feed.bump = ctx.bumps.feed;
        emit!(FeedConfigured {
            feed: feed.key(),
            asset_mint: feed.asset_mint,
            authority: feed.authority,
            params,
        });
        Ok(())
    }

    /// Change the feed's parameters (authority only; bounded, evented).
    pub fn set_params(ctx: Context<FeedAdmin>, params: FeedParams) -> Result<()> {
        params.validate()?;
        let feed = &mut ctx.accounts.feed;
        feed.params = params;
        emit!(FeedConfigured {
            feed: feed.key(),
            asset_mint: feed.asset_mint,
            authority: feed.authority,
            params,
        });
        Ok(())
    }

    /// Replace the publisher set (authority only; ME-P1).
    pub fn set_publishers(ctx: Context<FeedAdmin>, publishers: Vec<Pubkey>) -> Result<()> {
        require!(
            !publishers.is_empty() && publishers.len() <= MAX_PUBLISHERS,
            MarkError::BadPublisherSet
        );
        let feed = &mut ctx.accounts.feed;
        feed.publishers = [Pubkey::default(); MAX_PUBLISHERS];
        for (slot, p) in feed.publishers.iter_mut().zip(publishers.iter()) {
            require_keys_neq!(*p, Pubkey::default(), MarkError::BadPublisherSet);
            *slot = *p;
        }
        feed.n_publishers = publishers.len() as u8;
        emit!(PublishersChanged {
            feed: feed.key(),
            publishers,
        });
        Ok(())
    }

    /// Point the feed at the auction-engine print authority PDA allowed to
    /// report clearing prints (authority only; ME-P4).
    pub fn set_print_source(ctx: Context<FeedAdmin>, print_source: Pubkey) -> Result<()> {
        let feed = &mut ctx.accounts.feed;
        feed.print_source = print_source;
        emit!(PrintSourceChanged {
            feed: feed.key(),
            print_source,
        });
        Ok(())
    }

    /// ME-P1: publish a NAV computed off-chain from treasury holdings
    /// (`credit_math::weekly_publish`). Only a publisher-set key signs.
    pub fn publish_nav(ctx: Context<PublishNav>, publication: NavPublication) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let feed = &mut ctx.accounts.feed;
        require!(
            feed.is_publisher(&ctx.accounts.publisher.key()),
            MarkError::NotPublisher
        );
        write_nav(feed.key(), feed, publication, now, false)
    }

    /// AC-9 drill (devnet only, `drills_enabled` at init): the authority
    /// sets a mark directly. Evented with `admin_drill = true` so no print,
    /// export or demo can present it as a real NAV.
    pub fn publish_drill_nav(ctx: Context<FeedAdmin>, publication: NavPublication) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let feed = &mut ctx.accounts.feed;
        require!(feed.params.drills_enabled, MarkError::DrillsDisabled);
        write_nav(feed.key(), feed, publication, now, true)
    }

    /// ME-P2: event the state the clock has moved the feed into
    /// (permissionless; idempotent).
    pub fn refresh(ctx: Context<Refresh>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let feed = &mut ctx.accounts.feed;
        transition(feed.key(), feed, now);
        Ok(())
    }

    /// ME-P2/P7: Suspended is explicit only (authority).
    pub fn suspend(ctx: Context<FeedAdmin>, reason_hash: [u8; 32]) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let feed = &mut ctx.accounts.feed;
        feed.suspended = true;
        feed.suspend_reason = reason_hash;
        transition(feed.key(), feed, now);
        Ok(())
    }

    /// Lift a suspension (authority). The feed returns to its age state.
    pub fn resume(ctx: Context<FeedAdmin>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let feed = &mut ctx.accounts.feed;
        feed.suspended = false;
        feed.suspend_reason = [0; 32];
        transition(feed.key(), feed, now);
        Ok(())
    }

    /// ME-P4/P6: an auction session's clearing print. Only the feed's print
    /// source (auction-engine's print PDA, which signs via CPI) may report.
    /// One record per session; ineligible prints are stored, flagged, and
    /// never enter the implied curve.
    pub fn record_print(ctx: Context<RecordPrint>, print: PrintInput) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let feed = &mut ctx.accounts.feed;
        require_keys_eq!(
            ctx.accounts.print_source.key(),
            feed.print_source,
            MarkError::NotPrintSource
        );
        require!(print.price > 0 && print.volume > 0, MarkError::ZeroPrint);
        let price_pico = feed.price_to_pico(print.price)?;
        let eligibility = feed.print_eligibility(&print, price_pico);
        let accepted = eligibility == PrintEligibility::Eligible;
        if accepted {
            feed.implied_pico = price_pico;
            feed.implied_ts = now;
            feed.prints_accepted = feed.prints_accepted.saturating_add(1);
        } else {
            feed.prints_rejected = feed.prints_rejected.saturating_add(1);
        }
        let rec = &mut ctx.accounts.record;
        rec.feed = feed.key();
        rec.session = print.session;
        rec.price = print.price;
        rec.volume = print.volume;
        rec.n_investor_ids = print.n_investor_ids;
        rec.forced_sale = print.forced_sale;
        rec.eligibility = eligibility;
        rec.recorded_at = now;
        rec.bump = ctx.bumps.record;

        // ME-P6: what the print discloses depends on the market's class.
        let policy = feed.params.print_policy;
        let (disclosed_price, disclosed_volume) = match policy {
            PrintPolicy::Public => (Some(print.price), Some(print.volume)),
            PrintPolicy::Banded => (
                Some(band(print.price, feed.params.band_bps)),
                Some(print.volume),
            ),
            PrintPolicy::AggregateOnly => (None, Some(print.volume)),
            PrintPolicy::DataRoom => (None, None),
        };
        emit!(PrintRecorded {
            feed: feed.key(),
            session: print.session,
            policy,
            price: disclosed_price,
            volume: disclosed_volume,
            n_investor_ids: print.n_investor_ids,
            forced_sale: print.forced_sale,
            eligibility,
            implied_pico: feed.implied_pico,
        });
        Ok(())
    }
}

fn write_nav(key: Pubkey, feed: &mut Feed, p: NavPublication, now: i64, drill: bool) -> Result<()> {
    require!(!feed.suspended, MarkError::Suspended);
    require!(p.nav_pico > 0, MarkError::ZeroNav);
    require!(p.computed_ts <= now, MarkError::FutureNav);
    require!(p.computed_ts > feed.computed_ts, MarkError::NavNotNewer);
    feed.nav_pico = p.nav_pico;
    feed.computed_ts = p.computed_ts;
    feed.effective_ts = now;
    feed.model_version = p.model_version;
    feed.inputs_hash = p.inputs_hash;
    feed.publications = feed.publications.saturating_add(1);
    feed.last_drill = drill;
    emit!(NavPublished {
        feed: key,
        nav_pico: p.nav_pico,
        computed_ts: p.computed_ts,
        effective_ts: now,
        model_version: p.model_version,
        inputs_hash: p.inputs_hash,
        admin_drill: drill,
    });
    transition(key, feed, now);
    Ok(())
}

/// Event a state change once (ME-P2: "transitions evented").
fn transition(key: Pubkey, feed: &mut Feed, now: i64) {
    let state = feed.state_at(now);
    if state != feed.last_state {
        emit!(MarkStateChanged {
            feed: key,
            from: feed.last_state,
            to: state,
            breaker: state.breaker(),
            at: now,
        });
        feed.last_state = state;
    }
}

/// Round `price` down to its band (`band_bps` of the price, at least one
/// micro): BANDED prints disclose the band floor, never the exact price.
fn band(price: u64, band_bps: u64) -> u64 {
    let width = (u128::from(price) * u128::from(band_bps) / u128::from(BPS)).max(1) as u64;
    price / width * width
}

/// SPL Token and Token-2022 mints both carry `decimals` at byte 44.
fn mint_decimals(mint: &UncheckedAccount) -> Result<u8> {
    let owner = *mint.owner;
    require!(
        owner == TOKEN_PROGRAM_ID || owner == TOKEN_2022_PROGRAM_ID,
        MarkError::NotAMint
    );
    let data = mint.try_borrow_data()?;
    require!(data.len() >= 82 && data[45] == 1, MarkError::NotAMint);
    Ok(data[44])
}

pub const TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_2022_PROGRAM_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

// ── state ───────────────────────────────────────────────────────────────

/// ME-P2 mark states. `Suspended` is set only by `suspend`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum MarkState {
    Fresh,
    Warned,
    Stale,
    Suspended,
}

/// doc 06 §11 system breaker hierarchy (ME-P7).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum BreakerLevel {
    Normal,
    Cautious,
    Restricted,
    Standstill,
}

impl MarkState {
    pub fn breaker(self) -> BreakerLevel {
        match self {
            MarkState::Fresh => BreakerLevel::Normal,
            MarkState::Warned => BreakerLevel::Cautious,
            MarkState::Stale => BreakerLevel::Restricted,
            MarkState::Suspended => BreakerLevel::Standstill,
        }
    }
}

/// ME-P6 print policy classes.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum PrintPolicy {
    Public,
    Banded,
    AggregateOnly,
    DataRoom,
}

/// Why a print did or did not enter the implied curve (ME-P4).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum PrintEligibility {
    Eligible,
    ForcedSale,
    BelowMinVolume,
    TooFewInvestors,
    Outlier,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct FeedParams {
    /// Age at which a Fresh mark becomes Warned.
    pub warn_after: i64,
    /// Age at which the mark becomes Stale.
    pub stale_after: i64,
    /// Haircut applied while Warned (≤ `MAX_WARNED_HAIRCUT_BPS`).
    pub warned_haircut_bps: u64,
    /// Haircut applied while Stale or Suspended (≤ `MAX_STALE_HAIRCUT_BPS`).
    pub stale_haircut_bps: u64,
    pub print_policy: PrintPolicy,
    /// BANDED width, bps of price.
    pub band_bps: u64,
    /// Minimum print volume (base units) to enter the implied curve.
    pub min_print_volume: u64,
    /// Minimum distinct investor_ids on a print (G2).
    pub min_print_investors: u16,
    /// Maximum |print − NAV| / NAV for an eligible print.
    pub outlier_band_bps: u64,
    /// Devnet only: allow `publish_drill_nav` (AC-9 drill).
    pub drills_enabled: bool,
}

impl FeedParams {
    pub fn validate(&self) -> Result<()> {
        require!(
            self.warn_after >= MIN_WARN_SECONDS
                && self.warn_after < self.stale_after
                && self.stale_after <= MAX_STALE_SECONDS,
            MarkError::BadSchedule
        );
        require!(
            self.warned_haircut_bps <= MAX_WARNED_HAIRCUT_BPS
                && self.stale_haircut_bps <= MAX_STALE_HAIRCUT_BPS
                && self.warned_haircut_bps <= self.stale_haircut_bps,
            MarkError::HaircutAboveCeiling
        );
        require!(
            self.outlier_band_bps > 0 && self.outlier_band_bps <= MAX_OUTLIER_BAND_BPS,
            MarkError::BadOutlierBand
        );
        require!(
            self.band_bps > 0 && self.band_bps <= BPS,
            MarkError::BadBand
        );
        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavPublication {
    /// NAV at [`NAV_SCALE`] (`credit_math::nav_published_pico`).
    pub nav_pico: u128,
    /// When the publisher computed it (≤ now, strictly newer than the last).
    pub computed_ts: i64,
    pub model_version: u32,
    /// SHA-256 of the raw inputs (treasury snapshot, supply, fee accrual).
    pub inputs_hash: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrintInput {
    /// The auction-engine session that printed.
    pub session: Pubkey,
    /// Uniform clearing price, micro-USDC per whole asset unit.
    pub price: u64,
    /// Executed quantity, base units.
    pub volume: u64,
    pub n_investor_ids: u16,
    pub forced_sale: bool,
}

#[account]
#[derive(InitSpace)]
pub struct Feed {
    pub asset_mint: Pubkey,
    pub asset_decimals: u8,
    pub authority: Pubkey,
    /// auction-engine's print PDA (the only `record_print` signer).
    pub print_source: Pubkey,
    pub publishers: [Pubkey; MAX_PUBLISHERS],
    pub n_publishers: u8,
    pub params: FeedParams,
    /// Latest official NAV at [`NAV_SCALE`] (0 = never published).
    pub nav_pico: u128,
    pub computed_ts: i64,
    /// When the NAV was written on-chain; ages are measured from here.
    pub effective_ts: i64,
    pub model_version: u32,
    pub inputs_hash: [u8; 32],
    pub publications: u64,
    /// The latest NAV came from `publish_drill_nav`.
    pub last_drill: bool,
    pub suspended: bool,
    pub suspend_reason: [u8; 32],
    /// Last evented state (transitions are evented once).
    pub last_state: MarkState,
    /// Implied NAV from the latest eligible print (0 = none).
    pub implied_pico: u128,
    pub implied_ts: i64,
    pub prints_accepted: u32,
    pub prints_rejected: u32,
    pub bump: u8,
}

impl Feed {
    pub const SEED: &'static [u8] = b"feed";

    pub fn is_publisher(&self, key: &Pubkey) -> bool {
        self.publishers[..usize::from(self.n_publishers)].contains(key)
    }

    /// ME-P2: the state at `now`. Never published ⇒ Stale.
    pub fn state_at(&self, now: i64) -> MarkState {
        if self.suspended {
            return MarkState::Suspended;
        }
        if self.nav_pico == 0 {
            return MarkState::Stale;
        }
        let age = now.saturating_sub(self.effective_ts);
        if age < self.params.warn_after {
            MarkState::Fresh
        } else if age < self.params.stale_after {
            MarkState::Warned
        } else {
            MarkState::Stale
        }
    }

    /// ME-P3: the haircut the state carries.
    pub fn haircut_at(&self, now: i64) -> u64 {
        match self.state_at(now) {
            MarkState::Fresh => 0,
            MarkState::Warned => self.params.warned_haircut_bps,
            MarkState::Stale | MarkState::Suspended => self.params.stale_haircut_bps,
        }
    }

    /// ME-P3: `NAV_effective` (at [`NAV_SCALE`]).
    pub fn nav_effective(&self, now: i64) -> Result<u128> {
        haircut_bps(self.nav_pico, self.haircut_at(now), BPS).map_err(math)
    }

    /// The implied curve point, while it is no older than `stale_after`.
    pub fn implied_at(&self, now: i64) -> Option<u128> {
        (self.implied_pico > 0 && now.saturating_sub(self.implied_ts) < self.params.stale_after)
            .then_some(self.implied_pico)
    }

    /// ME-P5: `MARK_PROTECTIVE = min(NAV_effective, NAV_implied)`. What
    /// every consumer reads; never the raw NAV.
    pub fn mark_protective(&self, now: i64) -> Result<u128> {
        let eff = self.nav_effective(now)?;
        Ok(match self.implied_at(now) {
            Some(implied) => eff.min(implied),
            None => eff,
        })
    }

    /// LF-P4 / ME-P7: liquidation is allowed only on a Fresh or Warned mark.
    pub fn allows_liquidation(&self, now: i64) -> bool {
        matches!(self.state_at(now), MarkState::Fresh | MarkState::Warned)
    }

    /// LF-P4: new advances need a Fresh mark ("Warned blocks new advances").
    pub fn allows_new_advance(&self, now: i64) -> bool {
        self.state_at(now) == MarkState::Fresh
    }

    /// Price (micro-USDC per whole unit) → NAV scale.
    pub fn price_to_pico(&self, price_micros: u64) -> Result<u128> {
        mul_div(
            u128::from(price_micros),
            NAV_SCALE,
            10u128.pow(u32::from(self.asset_decimals)),
        )
        .map_err(math)
    }

    /// NAV scale → micro-USDC per whole unit (floored: a
    /// value used as a collateral mark or reserve rounds down).
    pub fn pico_to_price(&self, pico: u128) -> Result<u64> {
        let v =
            mul_div(pico, 10u128.pow(u32::from(self.asset_decimals)), NAV_SCALE).map_err(math)?;
        u64::try_from(v).map_err(|_| error!(MarkError::MathOverflow))
    }

    /// ME-P4 eligibility, in a fixed order.
    pub fn print_eligibility(&self, p: &PrintInput, price_pico: u128) -> PrintEligibility {
        if p.forced_sale {
            return PrintEligibility::ForcedSale;
        }
        if p.volume < self.params.min_print_volume {
            return PrintEligibility::BelowMinVolume;
        }
        if p.n_investor_ids < self.params.min_print_investors {
            return PrintEligibility::TooFewInvestors;
        }
        if self.nav_pico > 0 {
            let dev = price_pico.abs_diff(self.nav_pico);
            // dev / nav > band  ⇔  dev × 10⁴ > band × nav  (u128, no rounding).
            let lhs = dev.saturating_mul(u128::from(BPS));
            let rhs = self
                .nav_pico
                .saturating_mul(u128::from(self.params.outlier_band_bps));
            if lhs > rhs {
                return PrintEligibility::Outlier;
            }
        }
        PrintEligibility::Eligible
    }
}

/// One clearing print per auction session (ME-P4: rejected prints stored).
#[account]
#[derive(InitSpace)]
pub struct PrintRecord {
    pub feed: Pubkey,
    pub session: Pubkey,
    pub price: u64,
    pub volume: u64,
    pub n_investor_ids: u16,
    pub forced_sale: bool,
    pub eligibility: PrintEligibility,
    pub recorded_at: i64,
    pub bump: u8,
}

impl PrintRecord {
    pub const SEED: &'static [u8] = b"print";
}

fn math(_: credit_math::MathError) -> Error {
    error!(MarkError::MathOverflow)
}

// ── accounts ────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct InitializeFeed<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Feed::INIT_SPACE,
        seeds = [Feed::SEED, asset_mint.key().as_ref()],
        bump
    )]
    pub feed: Account<'info, Feed>,
    /// CHECK: an SPL Token or Token-2022 mint (owner + layout checked).
    pub asset_mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct FeedAdmin<'info> {
    #[account(
        mut,
        seeds = [Feed::SEED, feed.asset_mint.as_ref()],
        bump = feed.bump,
        has_one = authority @ MarkError::NotAuthority
    )]
    pub feed: Account<'info, Feed>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct PublishNav<'info> {
    #[account(mut, seeds = [Feed::SEED, feed.asset_mint.as_ref()], bump = feed.bump)]
    pub feed: Account<'info, Feed>,
    pub publisher: Signer<'info>,
}

#[derive(Accounts)]
pub struct Refresh<'info> {
    #[account(mut, seeds = [Feed::SEED, feed.asset_mint.as_ref()], bump = feed.bump)]
    pub feed: Account<'info, Feed>,
}

#[derive(Accounts)]
#[instruction(print: PrintInput)]
pub struct RecordPrint<'info> {
    #[account(mut, seeds = [Feed::SEED, feed.asset_mint.as_ref()], bump = feed.bump)]
    pub feed: Box<Account<'info, Feed>>,
    /// One record per session: a replayed print fails (account exists).
    #[account(
        init,
        payer = payer,
        space = 8 + PrintRecord::INIT_SPACE,
        seeds = [PrintRecord::SEED, feed.key().as_ref(), print.session.as_ref()],
        bump
    )]
    pub record: Box<Account<'info, PrintRecord>>,
    /// auction-engine's print PDA (signs via CPI).
    pub print_source: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

// ── events ──────────────────────────────────────────────────────────────

#[event]
pub struct FeedConfigured {
    pub feed: Pubkey,
    pub asset_mint: Pubkey,
    pub authority: Pubkey,
    pub params: FeedParams,
}

#[event]
pub struct PublishersChanged {
    pub feed: Pubkey,
    pub publishers: Vec<Pubkey>,
}

#[event]
pub struct PrintSourceChanged {
    pub feed: Pubkey,
    pub print_source: Pubkey,
}

#[event]
pub struct NavPublished {
    pub feed: Pubkey,
    pub nav_pico: u128,
    pub computed_ts: i64,
    pub effective_ts: i64,
    pub model_version: u32,
    pub inputs_hash: [u8; 32],
    /// AC-9 drill mark (`publish_drill_nav`), never a real NAV.
    pub admin_drill: bool,
}

#[event]
pub struct MarkStateChanged {
    pub feed: Pubkey,
    pub from: MarkState,
    pub to: MarkState,
    pub breaker: BreakerLevel,
    pub at: i64,
}

#[event]
pub struct PrintRecorded {
    pub feed: Pubkey,
    pub session: Pubkey,
    pub policy: PrintPolicy,
    /// Omitted for AGGREGATE_ONLY / DATA_ROOM; band floor for BANDED.
    pub price: Option<u64>,
    /// Omitted for DATA_ROOM.
    pub volume: Option<u64>,
    pub n_investor_ids: u16,
    pub forced_sale: bool,
    pub eligibility: PrintEligibility,
    pub implied_pico: u128,
}

// ── errors (append-only) ────────────────────────────────────────────────

#[error_code]
pub enum MarkError {
    #[msg("signer is not the feed authority")]
    NotAuthority,
    #[msg("signer is not in the feed's publisher set")]
    NotPublisher,
    #[msg("signer is not the feed's print source")]
    NotPrintSource,
    #[msg("publisher set must hold 1..=4 non-default keys")]
    BadPublisherSet,
    #[msg("warn_after/stale_after outside the hard bounds")]
    BadSchedule,
    #[msg("haircut above its hard on-chain ceiling")]
    HaircutAboveCeiling,
    #[msg("outlier band outside (0, 50%]")]
    BadOutlierBand,
    #[msg("band width outside (0, 100%]")]
    BadBand,
    #[msg("NAV must be positive")]
    ZeroNav,
    #[msg("computed_ts is in the future")]
    FutureNav,
    #[msg("computed_ts is not newer than the current mark")]
    NavNotNewer,
    #[msg("the feed is suspended")]
    Suspended,
    #[msg("drill marks are disabled on this feed")]
    DrillsDisabled,
    #[msg("print price and volume must be positive")]
    ZeroPrint,
    #[msg("account is not an SPL Token / Token-2022 mint")]
    NotAMint,
    #[msg("arithmetic overflow")]
    MathOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> FeedParams {
        FeedParams {
            warn_after: 8 * 86_400,
            stale_after: 15 * 86_400,
            warned_haircut_bps: 200,
            stale_haircut_bps: 1_000,
            print_policy: PrintPolicy::Public,
            band_bps: 100,
            min_print_volume: 1_000_000,
            min_print_investors: 3,
            outlier_band_bps: 2_000,
            drills_enabled: false,
        }
    }

    fn feed(nav: u128, at: i64) -> Feed {
        Feed {
            asset_mint: Pubkey::default(),
            asset_decimals: 6,
            authority: Pubkey::default(),
            print_source: Pubkey::default(),
            publishers: [Pubkey::default(); MAX_PUBLISHERS],
            n_publishers: 0,
            params: params(),
            nav_pico: nav,
            computed_ts: at,
            effective_ts: at,
            model_version: 1,
            inputs_hash: [0; 32],
            publications: 1,
            last_drill: false,
            suspended: false,
            suspend_reason: [0; 32],
            last_state: MarkState::Fresh,
            implied_pico: 0,
            implied_ts: 0,
            prints_accepted: 0,
            prints_rejected: 0,
            bump: 0,
        }
    }

    const ONE: u128 = 1_000_000_000_000; // 1 USDC per NPCS at NAV_SCALE

    #[test]
    fn states_age_fresh_warned_stale() {
        let f = feed(ONE, 0);
        assert_eq!(f.state_at(0), MarkState::Fresh);
        assert_eq!(f.state_at(8 * 86_400 - 1), MarkState::Fresh);
        assert_eq!(f.state_at(8 * 86_400), MarkState::Warned);
        assert_eq!(f.state_at(15 * 86_400 - 1), MarkState::Warned);
        assert_eq!(f.state_at(15 * 86_400), MarkState::Stale);
        assert_eq!(feed(0, 0).state_at(0), MarkState::Stale, "never published");
        let mut s = feed(ONE, 0);
        s.suspended = true;
        assert_eq!(s.state_at(0), MarkState::Suspended);
        assert_eq!(MarkState::Suspended.breaker(), BreakerLevel::Standstill);
    }

    #[test]
    fn haircuts_by_state_and_protective_min() {
        let mut f = feed(ONE, 0);
        assert_eq!(f.nav_effective(0).unwrap(), ONE);
        assert_eq!(f.nav_effective(9 * 86_400).unwrap(), ONE * 98 / 100);
        assert_eq!(f.nav_effective(16 * 86_400).unwrap(), ONE * 90 / 100);
        // Implied below official → protective = implied.
        f.implied_pico = ONE * 95 / 100;
        f.implied_ts = 0;
        assert_eq!(f.mark_protective(0).unwrap(), ONE * 95 / 100);
        // Implied above official → protective = official.
        f.implied_pico = ONE * 2;
        assert_eq!(f.mark_protective(0).unwrap(), ONE);
        // A stale implied point drops out.
        f.implied_pico = ONE / 2;
        assert_eq!(f.mark_protective(15 * 86_400).unwrap(), ONE * 90 / 100);
    }

    #[test]
    fn liquidation_and_advance_gates() {
        let mut f = feed(ONE, 0);
        assert!(f.allows_liquidation(0) && f.allows_new_advance(0));
        assert!(f.allows_liquidation(9 * 86_400) && !f.allows_new_advance(9 * 86_400));
        assert!(!f.allows_liquidation(16 * 86_400));
        f.suspended = true;
        assert!(!f.allows_liquidation(0) && !f.allows_new_advance(0));
    }

    #[test]
    fn print_eligibility_order() {
        let f = feed(ONE, 0);
        let mut p = PrintInput {
            session: Pubkey::default(),
            price: 1_000_000,
            volume: 5_000_000,
            n_investor_ids: 3,
            forced_sale: false,
        };
        let pico = f.price_to_pico(p.price).unwrap();
        assert_eq!(pico, ONE);
        assert_eq!(f.print_eligibility(&p, pico), PrintEligibility::Eligible);
        p.n_investor_ids = 2;
        assert_eq!(
            f.print_eligibility(&p, pico),
            PrintEligibility::TooFewInvestors
        );
        p.volume = 1;
        assert_eq!(
            f.print_eligibility(&p, pico),
            PrintEligibility::BelowMinVolume
        );
        p.forced_sale = true;
        assert_eq!(f.print_eligibility(&p, pico), PrintEligibility::ForcedSale);
        let p = PrintInput {
            forced_sale: false,
            volume: 5_000_000,
            n_investor_ids: 3,
            ..p
        };
        // 20% band: 0.80 eligible, 0.79 outlier.
        let at = |micros| f.price_to_pico(micros).unwrap();
        assert_eq!(
            f.print_eligibility(&p, at(800_000)),
            PrintEligibility::Eligible
        );
        assert_eq!(
            f.print_eligibility(&p, at(799_999)),
            PrintEligibility::Outlier
        );
    }

    #[test]
    fn price_conversions_round_down() {
        let mut f = feed(ONE, 0);
        assert_eq!(f.pico_to_price(ONE).unwrap(), 1_000_000);
        f.asset_decimals = 9;
        // 1 USDC per whole 9-dp token = 1e6 micros / 1e9 base × 1e12.
        assert_eq!(f.price_to_pico(1_000_000).unwrap(), 1_000_000_000);
        assert_eq!(f.pico_to_price(1_999_999).unwrap(), 1_999, "floors");
    }

    #[test]
    fn banding_and_param_bounds() {
        assert_eq!(band(1_234_567, 100), 1_234_567 / 12_345 * 12_345);
        assert_eq!(band(5, 100), 5, "width floors at one micro");
        assert!(params().validate().is_ok());
        let mut p = params();
        p.warned_haircut_bps = MAX_WARNED_HAIRCUT_BPS + 1;
        assert!(p.validate().is_err());
        let mut p = params();
        p.stale_after = p.warn_after;
        assert!(p.validate().is_err());
        let mut p = params();
        p.warn_after = MIN_WARN_SECONDS - 1;
        assert!(p.validate().is_err());
        let mut p = params();
        p.outlier_band_bps = 0;
        assert!(p.validate().is_err());
    }
}
