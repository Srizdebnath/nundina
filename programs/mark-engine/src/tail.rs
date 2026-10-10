//! The feed tail (#202): integrity state appended *behind* the [`Feed`]
//! struct in the same account, so the `Feed` layout every consumer
//! deserializes (auction-engine, liquidity-facility, queue-claim,
//! tranche-engine, the app and services) never changes.
//!
//! `Account<Feed>` reads the struct prefix and ignores trailing bytes, and
//! writes back only the prefix on exit, so the tail at
//! [`FeedTail::OFFSET`] is read and written here by hand. A feed created
//! before #202 has no tail until `migrate_feed` reallocs it; until then it
//! behaves exactly as before (no deviation bound, single publisher, no
//! attestation, implied = last eligible print).
//!
//! What lives here:
//! - [`GuardConfig`]: ME-P1 / G5 deviation bound, M-of-N quorum, optional
//!   SAS attestation of the NAV, and the implied-curve half-life (ME-P3/M5).
//! - [`PendingNav`]: a NAV awaiting co-signatures (quorum, or the co-sign
//!   that lets an out-of-band NAV through).
//! - [`NavReview`]: the authority's one-shot manual-review approval of one
//!   specific out-of-band publication.
//! - the implied ring: the last [`IMPLIED_RING`] eligible prints, volume
//!   weighted and time decayed into `Feed::implied_pico`.
//! - `last_state`, the full evented state including `Frozen` (the `Feed`
//!   copy stores `Frozen` as `Stale` so programs built before `Frozen`
//!   existed still decode the feed).

use anchor_lang::prelude::*;

use crate::{Feed, MarkError, MarkState, NavPublication, BPS, MAX_PUBLISHERS};

/// Eligible prints the implied curve remembers (ME-P3 / M5).
pub const IMPLIED_RING: usize = 8;
/// Tail layout version.
pub const TAIL_VERSION: u8 = 1;
/// Default per-publication NAV deviation bound for a new or migrated feed
/// (G5): a weekly private-credit NAV that moves 20% is a review, not a
/// publish. The authority may widen it (≤ 100%) or set 0 (unbounded).
pub const DEFAULT_MAX_NAV_DEVIATION_BPS: u64 = 2_000;

/// The Solana Attestation Service program (same id on devnet and mainnet).
pub const SAS_PROGRAM_ID: Pubkey = pubkey!("22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG");
/// SAS account discriminator of an `Attestation`.
pub const SAS_ATTESTATION_DISCRIMINATOR: u8 = 2;
/// SAS PDA seed of an attestation.
pub const SAS_ATTESTATION_SEED: &[u8] = b"attestation";

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct GuardConfig {
    /// G5: max |NAV − prior NAV| / prior NAV for a single-publisher
    /// publication, bps. Above it the NAV needs a second publisher's
    /// co-signature or the authority's `approve_nav_review`. 0 = unbounded.
    pub max_nav_deviation_bps: u64,
    /// ME-P1: publishers that must sign the identical publication (1 = any
    /// one publisher, the pre-#202 behaviour).
    pub quorum: u8,
    /// ME-P1: SAS credential of the NAV attestation. Default key = no
    /// attestation required.
    pub sas_credential: Pubkey,
    /// SAS schema (`nundina.nav.v1`) of the NAV attestation.
    pub sas_schema: Pubkey,
    /// ME-P3 / M5: an eligible print's weight halves every
    /// `implied_half_life` seconds. 0 = the feed's `warn_after`.
    pub implied_half_life: i64,
}

impl Default for GuardConfig {
    fn default() -> Self {
        GuardConfig {
            max_nav_deviation_bps: DEFAULT_MAX_NAV_DEVIATION_BPS,
            quorum: 1,
            sas_credential: Pubkey::default(),
            sas_schema: Pubkey::default(),
            implied_half_life: 0,
        }
    }
}

impl GuardConfig {
    pub fn validate(&self, n_publishers: u8) -> Result<()> {
        require!(self.max_nav_deviation_bps <= BPS, MarkError::BadGuard);
        require!(
            self.quorum >= 1
                && usize::from(self.quorum) <= MAX_PUBLISHERS
                && (n_publishers == 0 || self.quorum <= n_publishers),
            MarkError::BadGuard
        );
        require!(
            (self.sas_credential == Pubkey::default()) == (self.sas_schema == Pubkey::default()),
            MarkError::BadGuard
        );
        require!(self.implied_half_life >= 0, MarkError::BadGuard);
        Ok(())
    }

    pub fn attestation_required(&self) -> bool {
        self.sas_schema != Pubkey::default()
    }
}

/// A NAV awaiting co-signatures. `signers == 0` = none pending.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct PendingNav {
    pub publication: NavPublication,
    /// Bit i = publisher slot i signed this exact publication.
    pub signers: u8,
    pub proposed_at: i64,
}

impl PendingNav {
    pub const NONE: PendingNav = PendingNav {
        publication: NavPublication {
            nav_pico: 0,
            computed_ts: 0,
            model_version: 0,
            inputs_hash: [0; 32],
        },
        signers: 0,
        proposed_at: 0,
    };
}

/// The authority's approval of one specific out-of-band publication
/// (`nav_pico == 0` = none). Consumed by the publication it approves.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct NavReview {
    pub nav_pico: u128,
    pub computed_ts: i64,
}

/// One remembered eligible print (or, on a confidential feed, the single
/// folded implied point).
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq, InitSpace,
)]
pub struct ImpliedPoint {
    pub price_pico: u128,
    /// Volume (base units) at `ts`; decays with age.
    pub weight: u64,
    pub ts: i64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct FeedTail {
    pub version: u8,
    pub guard: GuardConfig,
    /// Last evented state, `Frozen` included.
    pub last_state: MarkState,
    pub review: NavReview,
    pub pending: PendingNav,
    pub ring_len: u8,
    /// Next slot to overwrite.
    pub ring_head: u8,
    pub ring: [ImpliedPoint; IMPLIED_RING],
    /// Room to append fields without another realloc.
    pub reserved: [u8; 64],
}

impl FeedTail {
    /// Byte offset of the tail: discriminator + the `Feed` struct.
    pub const OFFSET: usize = 8 + Feed::INIT_SPACE;
    /// Account size of a feed with a tail.
    pub const FEED_SPACE: usize = Self::OFFSET + Self::INIT_SPACE;

    pub fn new(last_state: MarkState) -> FeedTail {
        FeedTail {
            version: TAIL_VERSION,
            guard: GuardConfig::default(),
            last_state,
            review: NavReview {
                nav_pico: 0,
                computed_ts: 0,
            },
            pending: PendingNav::NONE,
            ring_len: 0,
            ring_head: 0,
            ring: [ImpliedPoint::default(); IMPLIED_RING],
            reserved: [0; 64],
        }
    }

    /// The tail of `feed`, or `None` on a feed not yet migrated.
    pub fn load(feed: &AccountInfo) -> Result<Option<FeedTail>> {
        let data = feed.try_borrow_data()?;
        if data.len() < Self::FEED_SPACE {
            return Ok(None);
        }
        let mut b: &[u8] = &data[Self::OFFSET..];
        let tail = FeedTail::deserialize(&mut b).map_err(|_| error!(MarkError::BadTail))?;
        require!(tail.version == TAIL_VERSION, MarkError::BadTail);
        Ok(Some(tail))
    }

    pub fn store(&self, feed: &AccountInfo) -> Result<()> {
        let mut data = feed.try_borrow_mut_data()?;
        require!(data.len() >= Self::FEED_SPACE, MarkError::FeedNotMigrated);
        let mut dst: &mut [u8] = &mut data[Self::OFFSET..];
        self.serialize(&mut dst)
            .map_err(|_| error!(MarkError::BadTail))
    }

    /// ME-P3 / M5: remember an eligible print and return the new implied
    /// NAV — the volume-weighted mean of the remembered prints, each
    /// print's volume halving every `half_life` seconds and dropping out
    /// once `max_age` old. `fold` (confidential print classes) collapses
    /// the ring into the single implied point afterwards, so the account
    /// never holds a per-print price for those classes.
    pub fn push_print(
        &mut self,
        price_pico: u128,
        volume: u64,
        now: i64,
        half_life: i64,
        max_age: i64,
        fold: bool,
    ) -> u128 {
        let head = usize::from(self.ring_head) % IMPLIED_RING;
        self.ring[head] = ImpliedPoint {
            price_pico,
            weight: volume,
            ts: now,
        };
        self.ring_head = ((head + 1) % IMPLIED_RING) as u8;
        self.ring_len = (usize::from(self.ring_len) + 1).min(IMPLIED_RING) as u8;
        let points = &self.ring[..usize::from(self.ring_len)];
        let (implied, weight) =
            implied_vwap(points, now, half_life, max_age).unwrap_or((price_pico, volume));
        if fold {
            self.ring = [ImpliedPoint::default(); IMPLIED_RING];
            self.ring[0] = ImpliedPoint {
                price_pico: implied,
                weight: weight.max(1),
                ts: now,
            };
            self.ring_len = 1;
            self.ring_head = 1;
        }
        implied
    }
}

/// `volume >> (age / half_life)`; 0 once `max_age` old.
pub fn decayed_weight(p: &ImpliedPoint, now: i64, half_life: i64, max_age: i64) -> u64 {
    let age = now.saturating_sub(p.ts).max(0);
    if age >= max_age {
        return 0;
    }
    if half_life <= 0 {
        return p.weight;
    }
    let halvings = age / half_life;
    if halvings >= 64 {
        0
    } else {
        p.weight >> halvings
    }
}

/// Σ price·w / Σ w over `points` (floored), with Σ w. `None` if every
/// weight decayed to zero or the sums overflow (the caller then uses the
/// newest print alone).
pub fn implied_vwap(
    points: &[ImpliedPoint],
    now: i64,
    half_life: i64,
    max_age: i64,
) -> Option<(u128, u64)> {
    let mut num: u128 = 0;
    let mut den: u128 = 0;
    for p in points {
        let w = u128::from(decayed_weight(p, now, half_life, max_age));
        if w == 0 {
            continue;
        }
        num = num.checked_add(p.price_pico.checked_mul(w)?)?;
        den = den.checked_add(w)?;
    }
    (den > 0).then(|| (num / den, u64::try_from(den).unwrap_or(u64::MAX)))
}

/// |new − old| / old in bps (ceiling, saturating). `old == 0` ⇒ 0.
pub fn deviation_bps(old: u128, new: u128) -> u64 {
    if old == 0 {
        return 0;
    }
    let dev = old.abs_diff(new);
    let bps = dev.saturating_mul(u128::from(BPS)).div_ceil(old);
    u64::try_from(bps).unwrap_or(u64::MAX)
}

// ── SAS NAV attestation (ME-P1) ─────────────────────────────────────────
//
// Minimal copy of credit-gate's strict SAS `Attestation` decode
// (`programs/credit-gate/src/eligibility.rs`); mark-engine does not depend
// on credit-gate.

/// A decoded SAS attestation account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SasAttestation {
    pub nonce: Pubkey,
    pub credential: Pubkey,
    pub schema: Pubkey,
    pub data: Vec<u8>,
    pub signer: Pubkey,
    pub expiry: i64,
    pub token_account: Pubkey,
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if b.len() < n {
        return None;
    }
    let (h, t) = b.split_at(n);
    *b = t;
    Some(h)
}

fn key(b: &mut &[u8]) -> Option<Pubkey> {
    Some(Pubkey::new_from_array(take(b, 32)?.try_into().ok()?))
}

impl SasAttestation {
    /// Strict decode: discriminator, then `nonce | credential | schema |
    /// u32-len data | signer | i64 expiry | token_account`, nothing after.
    pub fn parse(bytes: &[u8]) -> Option<SasAttestation> {
        let mut b = bytes;
        if *take(&mut b, 1)?.first()? != SAS_ATTESTATION_DISCRIMINATOR {
            return None;
        }
        let (nonce, credential, schema) = (key(&mut b)?, key(&mut b)?, key(&mut b)?);
        let len = u32::from_le_bytes(take(&mut b, 4)?.try_into().ok()?) as usize;
        let data = take(&mut b, len)?.to_vec();
        let signer = key(&mut b)?;
        let expiry = i64::from_le_bytes(take(&mut b, 8)?.try_into().ok()?);
        let token_account = key(&mut b)?;
        b.is_empty().then_some(SasAttestation {
            nonce,
            credential,
            schema,
            data,
            signer,
            expiry,
            token_account,
        })
    }

    /// The canonical SAS address of this attestation.
    pub fn address(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[
                SAS_ATTESTATION_SEED,
                self.credential.as_ref(),
                self.schema.as_ref(),
                self.nonce.as_ref(),
            ],
            &SAS_PROGRAM_ID,
        )
        .0
    }
}

/// `nundina.nav.v1` attestation data (SAS layout
/// `[VecU8, U128, I64, U32, VecU8]`): the asset mint (32 bytes), then the
/// publication exactly as `publish_nav` receives it.
pub fn nav_attestation_data(asset_mint: &Pubkey, p: &NavPublication) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + 32 + 16 + 8 + 4 + 4 + 32);
    v.extend_from_slice(&32u32.to_le_bytes());
    v.extend_from_slice(asset_mint.as_ref());
    v.extend_from_slice(&p.nav_pico.to_le_bytes());
    v.extend_from_slice(&p.computed_ts.to_le_bytes());
    v.extend_from_slice(&p.model_version.to_le_bytes());
    v.extend_from_slice(&32u32.to_le_bytes());
    v.extend_from_slice(&p.inputs_hash);
    v
}

/// ME-P1: `ai` is a live SAS attestation, at its canonical address, under
/// the feed's credential + schema, signed by a key in the publisher set,
/// unexpired, attesting exactly this publication of this asset.
pub fn verify_nav_attestation(
    ai: Option<&AccountInfo>,
    feed: &Feed,
    guard: &GuardConfig,
    p: &NavPublication,
    now: i64,
) -> Result<()> {
    let ai = ai.ok_or(error!(MarkError::AttestationMissing))?;
    require_keys_eq!(*ai.owner, SAS_PROGRAM_ID, MarkError::AttestationInvalid);
    let data = ai.try_borrow_data()?;
    let att = SasAttestation::parse(&data).ok_or(error!(MarkError::AttestationInvalid))?;
    let sound = att.credential == guard.sas_credential
        && att.schema == guard.sas_schema
        && feed.is_publisher(&att.signer)
        && att.data == nav_attestation_data(&feed.asset_mint, p)
        && att.address() == *ai.key;
    require!(sound, MarkError::AttestationInvalid);
    require!(
        att.expiry == 0 || att.expiry > now,
        MarkError::AttestationInvalid
    );
    Ok(())
}
