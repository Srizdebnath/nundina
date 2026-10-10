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
//!   Stale, Frozen or Suspended mark forbids liquidation for every consumer.
//!
//! #202 integrity (state in the feed *tail*, see [`tail`]; the `Feed`
//! struct consumers decode is unchanged):
//! - RISK: `drills_enabled` is fixed at `initialize_feed`; `set_params`
//!   cannot turn drills on (or off) later.
//! - ME-P1 / G5: a publication more than `max_nav_deviation_bps` from the
//!   prior NAV needs a second publisher's co-signature (same publication)
//!   or the authority's `approve_nav_review` of exactly that NAV.
//! - ME-P1: optional M-of-N `quorum`: the first signature records a
//!   pending NAV, the quorum-th identical signature writes it.
//! - ME-P1: optional SAS attestation of the NAV (`nundina.nav.v1`) as the
//!   first remaining account of `publish_nav`, when the feed configures a
//!   credential + schema.
//! - ME-P2: Frozen at 3× cadence (06 §4: Warned 1.25×, Stale 2×, Frozen
//!   3×), i.e. `stale_after × 3/2`; breaker Standstill.
//! - ME-P3 / M5: `NAV_implied` is the volume-weighted, time-decayed mean of
//!   the last [`tail::IMPLIED_RING`] eligible prints.
//! - ME-P6: `PrintRecord` stores the price / volume only as the class
//!   discloses them. Public-chain limit: the exact clearing price is still
//!   in the auction-engine session account, its `ClearingPrint` event and
//!   the `record_print` instruction data, and any implied NAV derived from
//!   a single print equals that print. Print classes are disclosure
//!   discipline in the product, not on-chain secrecy.

#![deny(unsafe_code)]
#![allow(missing_docs)]
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use credit_math::{haircut_bps, mul_div};

pub mod tail;
use tail::{FeedTail, GuardConfig, NavReview, PendingNav};

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
        // #202: new feeds carry the tail from birth (default guard).
        FeedTail::new(MarkState::Stale).store(&ctx.accounts.feed.to_account_info())
    }

    /// Change the feed's parameters (authority only; bounded, evented).
    /// `drills_enabled` is fixed at `initialize_feed` (RISK: a production
    /// feed can never be turned into a drill feed).
    pub fn set_params(ctx: Context<FeedAdmin>, params: FeedParams) -> Result<()> {
        params.validate()?;
        let feed = &mut ctx.accounts.feed;
        require!(
            params.drills_enabled == feed.params.drills_enabled,
            MarkError::DrillsImmutable
        );
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
        // Signatures are per publisher slot: a new set voids a pending NAV.
        let ai = ctx.accounts.feed.to_account_info();
        if let Some(mut t) = FeedTail::load(&ai)? {
            require!(
                t.guard.quorum <= ctx.accounts.feed.n_publishers,
                MarkError::QuorumAboveSet
            );
            t.pending = PendingNav::NONE;
            t.store(&ai)?;
        }
        emit!(PublishersChanged {
            feed: ctx.accounts.feed.key(),
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
    ///
    /// On a feed with a tail (#202): the optional SAS attestation of the
    /// NAV is the first remaining account; a publication outside the
    /// deviation bound, or on a feed with `quorum > 1`, is recorded as
    /// pending and written only when enough publishers have signed the
    /// identical publication (an out-of-band one needs at least two, or
    /// the authority's `approve_nav_review`).
    pub fn publish_nav<'info>(
        ctx: Context<'info, PublishNav<'info>>,
        publication: NavPublication,
    ) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let ai = ctx.accounts.feed.to_account_info();
        let key = ai.key();
        let signer = ctx.accounts.publisher.key();
        let feed = &mut ctx.accounts.feed;
        let slot = feed
            .publisher_slot(&signer)
            .ok_or(error!(MarkError::NotPublisher))?;
        let Some(mut t) = FeedTail::load(&ai)? else {
            // Not migrated: the pre-#202 single-publisher path.
            return write_nav(key, feed, &mut None, publication, now, false);
        };
        check_publication(feed, &publication, now)?;
        if t.guard.attestation_required() {
            tail::verify_nav_attestation(
                ctx.remaining_accounts.first(),
                feed,
                &t.guard,
                &publication,
                now,
            )?;
        }
        // G5: the bound is against the prior real NAV (none before the
        // first publication; a drill mark is no reference).
        let deviation = if feed.nav_pico > 0 && !feed.last_drill {
            tail::deviation_bps(feed.nav_pico, publication.nav_pico)
        } else {
            0
        };
        let out_of_band =
            t.guard.max_nav_deviation_bps > 0 && deviation > t.guard.max_nav_deviation_bps;
        let reviewed = out_of_band
            && t.review
                == (NavReview {
                    nav_pico: publication.nav_pico,
                    computed_ts: publication.computed_ts,
                });
        let mut required = t.guard.quorum.max(1);
        if out_of_band && !reviewed {
            required = required.max(2);
        }
        if required > feed.n_publishers {
            return Err(if out_of_band {
                error!(MarkError::NavDeviationTooLarge)
            } else {
                error!(MarkError::QuorumAboveSet)
            });
        }
        // Co-signatures accumulate on the identical publication only; a
        // different one (or an expired one) starts over.
        let live = t.pending.signers != 0
            && t.pending.publication == publication
            && now.saturating_sub(t.pending.proposed_at) < feed.params.warn_after;
        if !live {
            t.pending = PendingNav {
                publication,
                signers: 0,
                proposed_at: now,
            };
        }
        let bit = 1u8 << slot;
        require!(t.pending.signers & bit == 0, MarkError::AlreadySigned);
        t.pending.signers |= bit;
        let signatures = t.pending.signers.count_ones() as u8;
        emit!(NavSigned {
            feed: key,
            publisher: signer,
            nav_pico: publication.nav_pico,
            computed_ts: publication.computed_ts,
            signatures,
            required,
            deviation_bps: deviation,
            out_of_band,
            reviewed,
        });
        let mut tail = Some(t);
        if signatures >= required {
            if let Some(t) = tail.as_mut() {
                t.pending = PendingNav::NONE;
                if reviewed {
                    t.review = NavReview {
                        nav_pico: 0,
                        computed_ts: 0,
                    };
                }
            }
            write_nav(key, feed, &mut tail, publication, now, false)?;
        }
        tail.unwrap().store(&ai)
    }

    /// AC-9 drill (devnet only, `drills_enabled` at init): the authority
    /// sets a mark directly. Evented with `admin_drill = true` so no print,
    /// export or demo can present it as a real NAV.
    pub fn publish_drill_nav(ctx: Context<FeedAdmin>, publication: NavPublication) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let ai = ctx.accounts.feed.to_account_info();
        let feed = &mut ctx.accounts.feed;
        require!(feed.params.drills_enabled, MarkError::DrillsDisabled);
        let mut tail = FeedTail::load(&ai)?;
        write_nav(ai.key(), feed, &mut tail, publication, now, true)?;
        store_tail(&ai, &tail)
    }

    /// ME-P2: event the state the clock has moved the feed into
    /// (permissionless; idempotent).
    pub fn refresh(ctx: Context<Refresh>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let ai = ctx.accounts.feed.to_account_info();
        let mut tail = FeedTail::load(&ai)?;
        transition(ai.key(), &mut ctx.accounts.feed, &mut tail, now);
        store_tail(&ai, &tail)
    }

    /// ME-P2/P7: Suspended is explicit only (authority).
    pub fn suspend(ctx: Context<FeedAdmin>, reason_hash: [u8; 32]) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let ai = ctx.accounts.feed.to_account_info();
        let mut tail = FeedTail::load(&ai)?;
        let feed = &mut ctx.accounts.feed;
        feed.suspended = true;
        feed.suspend_reason = reason_hash;
        transition(ai.key(), feed, &mut tail, now);
        store_tail(&ai, &tail)
    }

    /// Lift a suspension (authority). The feed returns to its age state.
    pub fn resume(ctx: Context<FeedAdmin>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let ai = ctx.accounts.feed.to_account_info();
        let mut tail = FeedTail::load(&ai)?;
        let feed = &mut ctx.accounts.feed;
        feed.suspended = false;
        feed.suspend_reason = [0; 32];
        transition(ai.key(), feed, &mut tail, now);
        store_tail(&ai, &tail)
    }

    /// ME-P4/P6: an auction session's clearing print. Only the feed's print
    /// source (auction-engine's print PDA, which signs via CPI) may report.
    /// One record per session; ineligible prints are stored, flagged, and
    /// never enter the implied curve.
    pub fn record_print(ctx: Context<RecordPrint>, print: PrintInput) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let ai = ctx.accounts.feed.to_account_info();
        let mut tail = FeedTail::load(&ai)?;
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
        if accepted {
            feed.implied_pico = match tail.as_mut() {
                // ME-P3 / M5: the VWAP of recent eligible prints. A BANDED
                // feed remembers the band floor it discloses (never above
                // the exact price, so the protective mark only errs low);
                // AGGREGATE_ONLY / DATA_ROOM fold the ring to the derived
                // implied point so no per-print price is kept.
                Some(t) => {
                    let point = match disclosed_price {
                        Some(p) => feed.price_to_pico(p)?,
                        None => price_pico,
                    };
                    let half_life = if t.guard.implied_half_life > 0 {
                        t.guard.implied_half_life
                    } else {
                        feed.params.warn_after
                    };
                    t.push_print(
                        point,
                        print.volume,
                        now,
                        half_life,
                        feed.params.stale_after,
                        disclosed_price.is_none(),
                    )
                }
                None => price_pico,
            };
            feed.implied_ts = now;
            feed.prints_accepted = feed.prints_accepted.saturating_add(1);
        } else {
            feed.prints_rejected = feed.prints_rejected.saturating_add(1);
        }
        // The stored record holds only what the class discloses (0 =
        // withheld); the data room reads the auction session instead.
        let rec = &mut ctx.accounts.record;
        rec.feed = feed.key();
        rec.session = print.session;
        rec.price = disclosed_price.unwrap_or(0);
        rec.volume = disclosed_volume.unwrap_or(0);
        rec.n_investor_ids = print.n_investor_ids;
        rec.forced_sale = print.forced_sale;
        rec.eligibility = eligibility;
        rec.recorded_at = now;
        rec.bump = ctx.bumps.record;
        store_tail(&ai, &tail)?;
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

    /// #202: give a pre-#202 feed its tail (realloc, default guard).
    /// Permissionless — the payer funds the extra rent; idempotent.
    pub fn migrate_feed(ctx: Context<MigrateFeed>) -> Result<()> {
        let ai = ctx.accounts.feed.to_account_info();
        if ai.data_len() >= FeedTail::FEED_SPACE {
            return Ok(());
        }
        let need = Rent::get()?
            .minimum_balance(FeedTail::FEED_SPACE)
            .saturating_sub(ai.lamports());
        if need > 0 {
            anchor_lang::system_program::transfer(
                CpiContext::new(
                    ctx.accounts.system_program.key(),
                    anchor_lang::system_program::Transfer {
                        from: ctx.accounts.payer.to_account_info(),
                        to: ai.clone(),
                    },
                ),
                need,
            )?;
        }
        ai.resize(FeedTail::FEED_SPACE)?;
        let feed = &ctx.accounts.feed;
        FeedTail::new(feed.last_state).store(&ai)?;
        emit!(FeedMigrated {
            feed: ai.key(),
            space: FeedTail::FEED_SPACE as u32,
        });
        emit!(GuardConfigured {
            feed: ai.key(),
            guard: GuardConfig::default(),
        });
        Ok(())
    }

    /// ME-P1 / G5: set the deviation bound, quorum, NAV attestation and
    /// implied half-life (authority only; bounded, evented). Voids any
    /// pending NAV and review approval.
    pub fn set_guard(ctx: Context<FeedAdmin>, guard: GuardConfig) -> Result<()> {
        let ai = ctx.accounts.feed.to_account_info();
        let mut t = FeedTail::load(&ai)?.ok_or(error!(MarkError::FeedNotMigrated))?;
        guard.validate(ctx.accounts.feed.n_publishers)?;
        t.guard = guard;
        t.pending = PendingNav::NONE;
        t.review = NavReview {
            nav_pico: 0,
            computed_ts: 0,
        };
        t.store(&ai)?;
        emit!(GuardConfigured {
            feed: ai.key(),
            guard,
        });
        Ok(())
    }

    /// G5 manual review: the authority approves one specific out-of-band
    /// publication (`nav_pico`, `computed_ts`), which then needs only the
    /// quorum. One-shot; `nav_pico = 0` withdraws the approval.
    pub fn approve_nav_review(
        ctx: Context<FeedAdmin>,
        nav_pico: u128,
        computed_ts: i64,
    ) -> Result<()> {
        let ai = ctx.accounts.feed.to_account_info();
        let mut t = FeedTail::load(&ai)?.ok_or(error!(MarkError::FeedNotMigrated))?;
        t.review = NavReview {
            nav_pico,
            computed_ts,
        };
        t.store(&ai)?;
        emit!(NavReviewApproved {
            feed: ai.key(),
            nav_pico,
            computed_ts,
        });
        Ok(())
    }
}

fn store_tail(ai: &AccountInfo, tail: &Option<FeedTail>) -> Result<()> {
    match tail {
        Some(t) => t.store(ai),
        None => Ok(()),
    }
}

/// What every NAV write requires (checked before a NAV can be pending too).
fn check_publication(feed: &Feed, p: &NavPublication, now: i64) -> Result<()> {
    require!(!feed.suspended, MarkError::Suspended);
    require!(p.nav_pico > 0, MarkError::ZeroNav);
    require!(p.computed_ts <= now, MarkError::FutureNav);
    require!(p.computed_ts > feed.computed_ts, MarkError::NavNotNewer);
    Ok(())
}

fn write_nav(
    key: Pubkey,
    feed: &mut Feed,
    tail: &mut Option<FeedTail>,
    p: NavPublication,
    now: i64,
    drill: bool,
) -> Result<()> {
    check_publication(feed, &p, now)?;
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
    transition(key, feed, tail, now);
    Ok(())
}

/// Event a state change once (ME-P2: "transitions evented"). `Feed`
/// stores `Frozen` as `Stale` (programs built before `Frozen` existed must
/// still decode the feed); the tail keeps the full state. A feed without a
/// tail events Frozen once, on the transition out of Fresh/Warned.
fn transition(key: Pubkey, feed: &mut Feed, tail: &mut Option<FeedTail>, now: i64) {
    let state = feed.state_at(now);
    let from = match tail {
        Some(t) => t.last_state,
        None => feed.last_state,
    };
    let changed = match tail {
        Some(_) => state != from,
        None => state.stored() != from,
    };
    if changed {
        emit!(MarkStateChanged {
            feed: key,
            from,
            to: state,
            breaker: state.breaker(),
            at: now,
        });
        feed.last_state = state.stored();
        if let Some(t) = tail {
            t.last_state = state;
        }
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

/// ME-P2 mark states. `Suspended` is set only by `suspend`. `Frozen`
/// (appended, #202) is age > 3× cadence (06 §4).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum MarkState {
    Fresh,
    Warned,
    Stale,
    Suspended,
    Frozen,
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
            MarkState::Suspended | MarkState::Frozen => BreakerLevel::Standstill,
        }
    }

    /// The value `Feed::last_state` stores: `Frozen` as `Stale`.
    pub fn stored(self) -> MarkState {
        match self {
            MarkState::Frozen => MarkState::Stale,
            s => s,
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
    /// Devnet only: allow `publish_drill_nav` (AC-9 drill). Fixed at
    /// `initialize_feed`.
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

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
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
        self.publisher_slot(key).is_some()
    }

    /// The publisher-set slot of `key`.
    pub fn publisher_slot(&self, key: &Pubkey) -> Option<usize> {
        self.publishers[..usize::from(self.n_publishers).min(MAX_PUBLISHERS)]
            .iter()
            .position(|p| p == key)
    }

    /// ME-P2 / 06 §4: Frozen at 3× cadence. Stale is 2× cadence, so the
    /// Frozen age is `stale_after × 3/2`.
    pub fn frozen_after(&self) -> i64 {
        self.params
            .stale_after
            .saturating_add(self.params.stale_after / 2)
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
        } else if age < self.frozen_after() {
            MarkState::Stale
        } else {
            MarkState::Frozen
        }
    }

    /// ME-P7 / 06 §11: the mark puts the market in Standstill (Suspended
    /// or Frozen): auctions pause, queue-claims hold the last mark.
    pub fn is_standstill(&self, now: i64) -> bool {
        self.state_at(now).breaker() == BreakerLevel::Standstill
    }

    /// ME-P3: the haircut the state carries.
    pub fn haircut_at(&self, now: i64) -> u64 {
        match self.state_at(now) {
            MarkState::Fresh => 0,
            MarkState::Warned => self.params.warned_haircut_bps,
            MarkState::Stale | MarkState::Suspended | MarkState::Frozen => {
                self.params.stale_haircut_bps
            }
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
    /// As the class discloses it (ME-P6, #202): exact for PUBLIC, the band
    /// floor for BANDED, 0 (withheld) for AGGREGATE_ONLY / DATA_ROOM.
    /// Records written before #202 hold the exact price.
    pub price: u64,
    /// 0 (withheld) for DATA_ROOM.
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
        space = FeedTail::FEED_SPACE,
        seeds = [Feed::SEED, asset_mint.key().as_ref()],
        bump
    )]
    pub feed: Account<'info, Feed>,
    /// CHECK: an SPL Token or Token-2022 mint (owner + layout checked).
    pub asset_mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub authority: Signer<'info>,
    /// This program, to locate its ProgramData. The first caller of initialize_feed would own the feed: only the upgrade authority may create one.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, crate::program::MarkEngine>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ MarkError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,
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
pub struct MigrateFeed<'info> {
    #[account(mut, seeds = [Feed::SEED, feed.asset_mint.as_ref()], bump = feed.bump)]
    pub feed: Account<'info, Feed>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
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

/// A publisher signed a publication (#202). `signatures < required` ⇒
/// pending (quorum or out-of-band co-sign); otherwise `NavPublished`
/// follows in the same instruction.
#[event]
pub struct NavSigned {
    pub feed: Pubkey,
    pub publisher: Pubkey,
    pub nav_pico: u128,
    pub computed_ts: i64,
    pub signatures: u8,
    pub required: u8,
    /// |NAV − prior NAV| / prior NAV, bps (0 on the first or after a drill).
    pub deviation_bps: u64,
    pub out_of_band: bool,
    /// The authority's `approve_nav_review` covers this publication.
    pub reviewed: bool,
}

#[event]
pub struct GuardConfigured {
    pub feed: Pubkey,
    pub guard: GuardConfig,
}

#[event]
pub struct NavReviewApproved {
    pub feed: Pubkey,
    pub nav_pico: u128,
    pub computed_ts: i64,
}

#[event]
pub struct FeedMigrated {
    pub feed: Pubkey,
    pub space: u32,
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
    #[msg("Only the program's upgrade authority may do this")]
    NotUpgradeAuthority,
    #[msg("drills_enabled is fixed at initialize_feed")]
    DrillsImmutable,
    #[msg("NAV deviates beyond the bound: needs a co-signing publisher or a manual review")]
    NavDeviationTooLarge,
    #[msg("this publisher already signed the pending NAV")]
    AlreadySigned,
    #[msg("quorum exceeds the publisher set")]
    QuorumAboveSet,
    #[msg("guard config outside its bounds")]
    BadGuard,
    #[msg("feed has no #202 tail: run migrate_feed")]
    FeedNotMigrated,
    #[msg("feed tail is malformed")]
    BadTail,
    #[msg("NAV attestation account missing")]
    AttestationMissing,
    #[msg("NAV attestation invalid, expired, or not for this publication")]
    AttestationInvalid,
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
