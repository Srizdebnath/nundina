//! CG-P3 — the per-holder lot ledger (compressed Rule-144 analog, CG-1;
//! doc 09 §2; #122).
//!
//! One [`HolderLots`] PDA per `(mint, holder)`: a fixed-capacity set of
//! seasoning lots plus a FREE bucket. A lot is sellable once
//! `restriction_expiry ≤ now`; `merge_expired` (permissionless) folds
//! expired lots into FREE so the ledger never fills with dead entries.
//!
//! Who may write:
//! - `open_holder_lots`: anyone may pay to open a ledger, but only for a
//!   wallet that passes the block-list screen (CG-P5 precedence);
//! - `append_lot` / `split_lot`: the market's `gate_config_authority`.
//!   SRS 08 names auction-engine settle and the primary mint as the callers;
//!   neither exists yet (#128), so the issuer's governing key stands in.
//!   `append_lot` re-screens the holder every time;
//! - `merge_expired`: anyone. There is no crank fee yet (SRS 08 CG-P3 asks
//!   for one); the call only moves expired quantity into FREE.
//!
//! What this does not do yet: decrement on sale (the seller side of a fill
//! lands with auction-engine's settle path), holder-count bookkeeping
//! (CG-P4 needs `investor_id` from SAS, #52) and the Token ACL thaw (CG-P1).

use anchor_lang::prelude::*;

use crate::blocklist::BlockEntry;
use crate::{GateError, MarketConfig};

/// Lot slots per holder per mint (the D15 boundary). A full ledger first
/// compacts its expired lots; only a ledger full of *unexpired* lots rejects
/// a new one ([`GateError::LotLedgerFull`]).
pub const MAX_LOTS: usize = 16;

/// Where a lot came from. Wire-stable; append only.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, AnchorSerialize, AnchorDeserialize, InitSpace,
)]
pub enum LotSource {
    /// Issued by the issuer (primary mint).
    #[default]
    PrimaryMint,
    /// Bought in an auction session (auction-engine settle).
    AuctionSettle,
}

/// One seasoning lot. A split keeps every field but `lot_id` and `qty`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, AnchorSerialize, AnchorDeserialize, InitSpace,
)]
pub struct Lot {
    /// Ledger-local id, never reused.
    pub lot_id: u32,
    /// Base units.
    pub qty: u64,
    /// When the holder acquired it.
    pub acquired_ts: i64,
    /// Sellable at or after this time: `acquired_ts + seasoning_seconds`.
    pub restriction_expiry: i64,
    /// Primary mint or auction.
    pub source: LotSource,
    /// Acquired by an affiliate of the issuer (doc 07: affiliate resale
    /// rules differ).
    pub affiliate: bool,
    /// Exemption the acquisition relied on (code; mapping off-chain, like
    /// `GateReject`).
    pub exemption_path: u8,
}

/// The ledger for one `(mint, holder)`.
#[account]
#[derive(InitSpace)]
pub struct HolderLots {
    /// The gated mint.
    pub mint: Pubkey,
    /// The holder wallet.
    pub holder: Pubkey,
    /// Seasoned quantity, no longer tracked per lot.
    pub free_qty: u64,
    /// Next `lot_id` to assign.
    pub next_lot_id: u32,
    /// Live entries in `lots`, packed at the front.
    pub lot_count: u8,
    /// Seasoning lots; `lots[lot_count..]` are zeroed.
    pub lots: [Lot; MAX_LOTS],
    /// PDA bump.
    pub bump: u8,
}

/// Emitted for every lot-ledger change (CG-P3, CG-P7). Same family as the
/// skeleton's `LotEvent`, extended with the lot id and holder FREE balance.
#[event]
#[derive(Debug)]
pub struct LotEvent {
    /// 0 = append, 1 = split, 2 = merge-to-free.
    pub kind: u8,
    /// Gated mint.
    pub mint: Pubkey,
    /// Lot owner.
    pub owner: Pubkey,
    /// Append: the new lot. Split: the new (split-off) lot. Merge: 0.
    pub lot_id: u32,
    /// Quantity affected (base units): appended, split off, or merged.
    pub qty: u64,
    /// Append/split: the lot's seasoning expiry. Merge: the merge time.
    pub restriction_expiry: i64,
    /// Holder's FREE bucket after this change.
    pub free_qty: u64,
}

impl HolderLots {
    /// PDA seed prefix: `["lots", mint, holder]`.
    pub const SEED: &'static [u8] = b"lots";

    /// Live lots.
    pub fn live(&self) -> &[Lot] {
        &self.lots[..usize::from(self.lot_count)]
    }

    /// Everything the holder has on this ledger (FREE + every lot).
    pub fn total(&self) -> Result<u64> {
        self.live()
            .iter()
            .try_fold(self.free_qty, |a, l| a.checked_add(l.qty))
            .ok_or_else(|| error!(GateError::MathOverflow))
    }

    /// What the holder may sell at `now`: FREE plus every expired lot (AC-6).
    pub fn sellable(&self, now: i64) -> Result<u64> {
        self.live()
            .iter()
            .filter(|l| l.restriction_expiry <= now)
            .try_fold(self.free_qty, |a, l| a.checked_add(l.qty))
            .ok_or_else(|| error!(GateError::MathOverflow))
    }

    /// Fold every lot with `restriction_expiry ≤ now` into FREE and pack the
    /// rest, keeping their order. Returns the quantity merged.
    pub fn merge_expired(&mut self, now: i64) -> Result<u64> {
        let mut merged = 0u64;
        let mut kept = 0usize;
        for i in 0..usize::from(self.lot_count) {
            let lot = self.lots[i];
            if lot.restriction_expiry <= now {
                merged = merged.checked_add(lot.qty).ok_or(GateError::MathOverflow)?;
            } else {
                self.lots[kept] = lot;
                kept += 1;
            }
        }
        self.lots[kept..].fill(Lot::default());
        self.lot_count = kept as u8;
        self.free_qty = self
            .free_qty
            .checked_add(merged)
            .ok_or(GateError::MathOverflow)?;
        Ok(merged)
    }

    fn push(&mut self, mut lot: Lot, now: i64) -> Result<Lot> {
        if usize::from(self.lot_count) == MAX_LOTS {
            self.merge_expired(now)?;
        }
        require!(
            usize::from(self.lot_count) < MAX_LOTS,
            GateError::LotLedgerFull
        );
        lot.lot_id = self.next_lot_id;
        self.next_lot_id = self
            .next_lot_id
            .checked_add(1)
            .ok_or(GateError::MathOverflow)?;
        self.lots[usize::from(self.lot_count)] = lot;
        self.lot_count += 1;
        self.total()?; // the ledger's sum must stay representable
        Ok(lot)
    }

    /// Append a lot acquired at `now`, seasoning for `seasoning_seconds`.
    pub fn append(
        &mut self,
        qty: u64,
        now: i64,
        seasoning_seconds: i64,
        source: LotSource,
        affiliate: bool,
        exemption_path: u8,
    ) -> Result<Lot> {
        require!(qty > 0, GateError::InvalidLotQty);
        let restriction_expiry = now
            .checked_add(seasoning_seconds)
            .ok_or(GateError::MathOverflow)?;
        self.push(
            Lot {
                lot_id: 0,
                qty,
                acquired_ts: now,
                restriction_expiry,
                source,
                affiliate,
                exemption_path,
            },
            now,
        )
    }

    /// Split `qty` off lot `lot_id` into a new lot with the same metadata
    /// (CG-P3: a partial fill must not reset or lose seasoning). Returns
    /// `(remaining original, new lot)`.
    pub fn split(&mut self, lot_id: u32, qty: u64, now: i64) -> Result<(Lot, Lot)> {
        let idx = self
            .live()
            .iter()
            .position(|l| l.lot_id == lot_id)
            .ok_or(GateError::LotNotFound)?;
        let original = self.lots[idx];
        require!(qty > 0 && qty < original.qty, GateError::InvalidLotQty);
        // Make room first: compaction may move the original.
        if usize::from(self.lot_count) == MAX_LOTS {
            self.merge_expired(now)?;
        }
        let idx = self
            .live()
            .iter()
            .position(|l| l.lot_id == lot_id)
            // The original was itself expired and merged into FREE: there
            // is nothing seasoning left to split.
            .ok_or(GateError::LotNotFound)?;
        let new = self.push(Lot { qty, ..original }, now)?;
        self.lots[idx].qty = original.qty - qty;
        Ok((self.lots[idx], new))
    }
}

#[derive(Accounts)]
#[instruction(holder: Pubkey)]
pub struct OpenHolderLots<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        init,
        payer = payer,
        space = 8 + HolderLots::INIT_SPACE,
        seeds = [HolderLots::SEED, mint.key().as_ref(), holder.as_ref()],
        bump
    )]
    pub lots: Account<'info, HolderLots>,
    /// CHECK: the holder's block entry address, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), holder.as_ref()], bump)]
    pub block_entry: UncheckedAccount<'info>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AppendLot<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), lots.holder.as_ref()],
        bump = lots.bump
    )]
    pub lots: Account<'info, HolderLots>,
    /// CHECK: the holder's block entry address, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), lots.holder.as_ref()], bump)]
    pub block_entry: UncheckedAccount<'info>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    pub gate_config_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct SplitLot<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), lots.holder.as_ref()],
        bump = lots.bump
    )]
    pub lots: Account<'info, HolderLots>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    pub gate_config_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct MergeExpired<'info> {
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), lots.holder.as_ref()],
        bump = lots.bump
    )]
    pub lots: Account<'info, HolderLots>,
    /// CHECK: mint key must derive the ledger PDA.
    pub mint: UncheckedAccount<'info>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> HolderLots {
        HolderLots {
            mint: Pubkey::new_from_array([1; 32]),
            holder: Pubkey::new_from_array([2; 32]),
            free_qty: 0,
            next_lot_id: 0,
            lot_count: 0,
            lots: [Lot::default(); MAX_LOTS],
            bump: 255,
        }
    }

    const DAY: i64 = 86_400;

    #[test]
    fn append_seasons_from_acquisition() {
        let mut h = ledger();
        let lot = h
            .append(500, 1_000, 7 * DAY, LotSource::AuctionSettle, true, 3)
            .unwrap();
        assert_eq!((lot.lot_id, lot.qty), (0, 500));
        assert_eq!(lot.acquired_ts, 1_000);
        assert_eq!(lot.restriction_expiry, 1_000 + 7 * DAY);
        assert_eq!(h.sellable(1_000 + 7 * DAY - 1).unwrap(), 0);
        assert_eq!(h.sellable(1_000 + 7 * DAY).unwrap(), 500);
        assert_eq!(h.total().unwrap(), 500);
        assert!(h
            .append(0, 1_000, DAY, LotSource::PrimaryMint, false, 0)
            .is_err());
        assert!(h
            .append(1, i64::MAX, 1, LotSource::PrimaryMint, false, 0)
            .is_err());
    }

    #[test]
    fn split_preserves_metadata_and_quantity() {
        let mut h = ledger();
        h.append(500, 10, DAY, LotSource::AuctionSettle, true, 7)
            .unwrap();
        let (rest, new) = h.split(0, 200, 10).unwrap();
        assert_eq!(
            (rest.lot_id, rest.qty, new.lot_id, new.qty),
            (0, 300, 1, 200)
        );
        for l in [rest, new] {
            assert_eq!(
                (
                    l.acquired_ts,
                    l.restriction_expiry,
                    l.source,
                    l.affiliate,
                    l.exemption_path
                ),
                (10, 10 + DAY, LotSource::AuctionSettle, true, 7)
            );
        }
        assert_eq!(h.total().unwrap(), 500);
        assert!(h.split(0, 0, 10).is_err());
        assert!(
            h.split(0, 300, 10).is_err(),
            "a split must leave both halves non-empty"
        );
        assert!(h.split(9, 1, 10).is_err());
    }

    #[test]
    fn merge_expired_moves_only_expired_lots_and_packs() {
        let mut h = ledger();
        h.append(1, 0, DAY, LotSource::PrimaryMint, false, 0)
            .unwrap(); // exp DAY
        h.append(2, 0, 3 * DAY, LotSource::PrimaryMint, false, 0)
            .unwrap(); // exp 3·DAY
        h.append(4, 0, 2 * DAY, LotSource::PrimaryMint, false, 0)
            .unwrap(); // exp 2·DAY
        assert_eq!(h.merge_expired(DAY - 1).unwrap(), 0);
        assert_eq!(h.merge_expired(2 * DAY).unwrap(), 5);
        assert_eq!(h.free_qty, 5);
        assert_eq!(h.live().iter().map(|l| l.qty).collect::<Vec<_>>(), vec![2]);
        assert_eq!(h.lots[1], Lot::default(), "tail is zeroed");
        assert_eq!(h.total().unwrap(), 7);
    }

    #[test]
    fn full_ledger_compacts_expired_then_rejects() {
        let mut h = ledger();
        for i in 0..MAX_LOTS as i64 {
            h.append(1, i, DAY, LotSource::PrimaryMint, false, 0)
                .unwrap();
        }
        // Nothing expired: full.
        assert!(h
            .append(1, 0, DAY, LotSource::PrimaryMint, false, 0)
            .is_err());
        assert!(h.split(0, 1, 0).is_err());
        // Lot 0 expires at DAY: one slot frees up on the next append.
        let lot = h
            .append(9, DAY, DAY, LotSource::PrimaryMint, false, 0)
            .unwrap();
        assert_eq!(lot.lot_id, MAX_LOTS as u32, "ids are never reused");
        assert_eq!((h.free_qty, usize::from(h.lot_count)), (1, MAX_LOTS));
    }

    #[test]
    fn split_of_a_lot_merged_away_is_not_found() {
        let mut h = ledger();
        for i in 0..MAX_LOTS as i64 {
            h.append(2, i, DAY, LotSource::PrimaryMint, false, 0)
                .unwrap();
        }
        // At DAY, lot 0 expired: making room merges it into FREE.
        assert!(h.split(0, 1, DAY).is_err());
        assert_eq!(h.free_qty, 2);
        // A live lot can still be split into the freed slot.
        let (rest, new) = h.split(5, 1, DAY).unwrap();
        assert_eq!((rest.qty, new.qty, new.restriction_expiry), (1, 1, 5 + DAY));
    }
}
