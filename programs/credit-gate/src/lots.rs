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
//! - `settle_check` ([`crate::settle`]): payment-rails' settlement PDA, once
//!   per fill (AE-P8, #133). It debits the seller's seasoned quantity
//!   ([`HolderLots::debit`]) and appends the buyer's new lot.

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
    /// Bought out of custody: a liquidity-facility ladder sale (LF-P5).
    CustodySale,
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

/// Most lots one sale may draw from, FREE included. Equal to payment-rails'
/// `MAX_LOTS_PER_FILL`, the cap on a `TransferInstruction`'s `lot_ids`.
pub const MAX_DRAWS: usize = 8;

/// The `lot_id` a [`LotDraw`] from the FREE bucket carries. FREE quantity
/// was seasoned lots that `merge_expired` folded together, so it has no
/// single acquisition time; its draw reports `acquired_ts` and
/// `restriction_expiry` as 0.
pub const FREE_LOT_ID: u32 = u32::MAX;

/// Quantity a sale took from one lot (or from FREE, [`FREE_LOT_ID`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub struct LotDraw {
    pub lot_id: u32,
    pub qty: u64,
    pub acquired_ts: i64,
    pub restriction_expiry: i64,
}

/// Emitted for every lot-ledger change (CG-P3, CG-P7). Same family as the
/// skeleton's `LotEvent`, extended with the lot id and holder FREE balance.
#[event]
#[derive(Debug)]
pub struct LotEvent {
    /// 0 = append, 1 = split, 2 = merge-to-free. A sale's debit is evented
    /// as [`crate::SettleChecked`], which lists every lot it drew from.
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

    /// Take `qty` for a sale at `now` (AE-P8, C2). Only seasoned quantity
    /// can go: FREE plus every lot with `restriction_expiry ≤ now`. Fails
    /// [`GateError::SeasoningNotMet`] if that is short of `qty`, whatever
    /// the unseasoned lots hold.
    ///
    /// Seasoned lots are drawn oldest acquisition first, each one a
    /// [`LotDraw`], so the transfer record names the lots that went. At
    /// most `MAX_DRAWS - 1` lots are named; past that, and when the named
    /// lots run out, every seasoned lot is merged into FREE and the rest
    /// comes from FREE as a single last draw. Emptied lots leave the ring.
    pub fn debit(&mut self, qty: u64, now: i64) -> Result<Vec<LotDraw>> {
        require!(qty > 0, GateError::InvalidLotQty);
        require!(self.sellable(now)? >= qty, GateError::SeasoningNotMet);
        let mut seasoned: Vec<usize> = (0..usize::from(self.lot_count))
            .filter(|&i| self.lots[i].restriction_expiry <= now)
            .collect();
        seasoned.sort_by_key(|&i| (self.lots[i].acquired_ts, self.lots[i].lot_id));
        let mut draws = Vec::new();
        let mut left = qty;
        for &i in seasoned.iter().take(MAX_DRAWS - 1) {
            let lot = &mut self.lots[i];
            let take = left.min(lot.qty);
            lot.qty -= take;
            left -= take;
            draws.push(LotDraw {
                lot_id: lot.lot_id,
                qty: take,
                acquired_ts: lot.acquired_ts,
                restriction_expiry: lot.restriction_expiry,
            });
            if left == 0 {
                break;
            }
        }
        if left > 0 {
            self.merge_expired(now)?;
            // `sellable ≥ qty` above: FREE now holds at least `left`.
            self.free_qty -= left;
            draws.push(LotDraw {
                lot_id: FREE_LOT_ID,
                qty: left,
                acquired_ts: 0,
                restriction_expiry: 0,
            });
        }
        self.drop_empty();
        Ok(draws)
    }

    /// CG-P3 custody (LF-P5): take `qty` off this ledger *with* its
    /// seasoning, for another ledger to [`deposit`](Self::deposit). FREE
    /// goes first, then lots by acquisition (oldest first); a lot only
    /// partly taken stays with its remainder. Unlike [`debit`](Self::debit)
    /// this moves unseasoned quantity too: nothing is sold, the units keep
    /// their acquisition dates (a pledge, its release, a workout). Returns
    /// the FREE quantity and the lots taken (`lot_id`s are reassigned by
    /// the receiving ledger).
    pub fn withdraw(&mut self, qty: u64) -> Result<(u64, Vec<Lot>)> {
        require!(qty > 0, GateError::InvalidLotQty);
        require!(self.total()? >= qty, GateError::InsufficientLots);
        let free = qty.min(self.free_qty);
        self.free_qty -= free;
        let mut left = qty - free;
        let mut order: Vec<usize> = (0..usize::from(self.lot_count)).collect();
        order.sort_by_key(|&i| (self.lots[i].acquired_ts, self.lots[i].lot_id));
        let mut taken = Vec::new();
        for i in order {
            if left == 0 {
                break;
            }
            let lot = self.lots[i];
            let take = left.min(lot.qty);
            self.lots[i].qty -= take;
            left -= take;
            taken.push(Lot { qty: take, ..lot });
        }
        // `total() ≥ qty` above: the lots covered the rest.
        self.drop_empty();
        Ok((free, taken))
    }

    /// Receive what [`withdraw`](Self::withdraw) took from another ledger:
    /// FREE into FREE, each lot with its own acquisition date and expiry.
    pub fn deposit(&mut self, free: u64, lots: &[Lot], now: i64) -> Result<()> {
        self.free_qty = self
            .free_qty
            .checked_add(free)
            .ok_or(GateError::MathOverflow)?;
        for lot in lots {
            self.push(*lot, now)?;
        }
        self.total()?;
        Ok(())
    }

    /// Remove zero-quantity lots, keeping the order of the rest.
    fn drop_empty(&mut self) {
        let mut kept = 0usize;
        for i in 0..usize::from(self.lot_count) {
            if self.lots[i].qty > 0 {
                self.lots[kept] = self.lots[i];
                kept += 1;
            }
        }
        self.lots[kept..].fill(Lot::default());
        self.lot_count = kept as u8;
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

    /// The byte layout `payment-rails/crank/src/seasoning.ts` decodes to
    /// find ledgers with seasoned lots (#127). Changing a field here must
    /// change the crank in the same commit.
    #[test]
    fn layout_read_by_the_seasoning_crank() {
        use anchor_lang::AccountSerialize;
        assert_eq!(8 + HolderLots::INIT_SPACE, 582);
        let mut h = ledger();
        h.append(7, 100, 50, LotSource::AuctionSettle, true, 3)
            .unwrap();
        h.append(9, 200, 50, LotSource::PrimaryMint, false, 0)
            .unwrap();
        let mut buf = Vec::new();
        h.try_serialize(&mut buf).unwrap();
        assert_eq!(buf.len(), 582);
        assert_eq!(&buf[8..40], h.mint.as_ref(), "mint at 8");
        assert_eq!(buf[84], 2, "lot_count at 84");
        let lot = |i: usize| 85 + 31 * i;
        let at = |o: usize| i64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
        assert_eq!(at(lot(0) + 20), 150, "lot 0 restriction_expiry at +20");
        assert_eq!(at(lot(1) + 20), 250, "lot 1 restriction_expiry at +20");
    }

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
    fn debit_takes_only_seasoned_quantity_oldest_first() {
        let mut h = ledger();
        h.append(5, 30, DAY, LotSource::PrimaryMint, false, 0)
            .unwrap(); // id 0, exp 30+DAY
        h.append(7, 10, DAY, LotSource::PrimaryMint, false, 0)
            .unwrap(); // id 1, exp 10+DAY (older)
        h.append(100, 50, 10 * DAY, LotSource::AuctionSettle, false, 1)
            .unwrap(); // id 2, unseasoned until 50+10·DAY
        let now = 30 + DAY;
        // 12 seasoned, 100 not: a sale of 13 fails however much is held.
        assert_eq!(h.total().unwrap(), 112);
        assert!(h.debit(13, now).is_err());
        assert!(h.debit(0, now).is_err());
        let draws = h.debit(9, now).unwrap();
        assert_eq!(
            draws
                .iter()
                .map(|d| (d.lot_id, d.qty, d.acquired_ts))
                .collect::<Vec<_>>(),
            vec![(1, 7, 10), (0, 2, 30)],
            "oldest acquisition first"
        );
        // Lot 1 is gone, lot 0 keeps 3, the unseasoned lot is untouched.
        assert_eq!(
            h.live()
                .iter()
                .map(|l| (l.lot_id, l.qty))
                .collect::<Vec<_>>(),
            vec![(0, 3), (2, 100)]
        );
        assert_eq!(h.total().unwrap(), 103);
        assert_eq!(h.lots[2], Lot::default(), "tail is zeroed");
    }

    #[test]
    fn debit_falls_back_to_free_and_caps_named_draws() {
        let mut h = ledger();
        h.free_qty = 4;
        for i in 0..10 {
            h.append(1, i, DAY, LotSource::PrimaryMint, false, 0)
                .unwrap();
        }
        let now = 10 + DAY;
        // 10 seasoned lots of 1 + FREE 4: sell 12. Seven named draws, then
        // the other three lots merge into FREE (4 + 3 = 7) and 5 comes from it.
        let draws = h.debit(12, now).unwrap();
        assert_eq!(draws.len(), MAX_DRAWS);
        assert!(draws[..7].iter().all(|d| d.qty == 1 && d.lot_id < 7));
        assert_eq!(
            (draws[7].lot_id, draws[7].qty, draws[7].restriction_expiry),
            (FREE_LOT_ID, 5, 0)
        );
        assert_eq!((h.free_qty, h.lot_count), (2, 0));
        assert_eq!(h.total().unwrap(), 2);
        // Only FREE left: one draw.
        let draws = h.debit(2, now).unwrap();
        assert_eq!(
            draws.iter().map(|d| (d.lot_id, d.qty)).collect::<Vec<_>>(),
            vec![(FREE_LOT_ID, 2)]
        );
        assert!(h.debit(1, now).is_err(), "nothing left");
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

    /// LF-P5: a pledge moves FREE first, then the oldest lots, keeping each
    /// lot's dates; a lot only partly taken keeps its remainder.
    #[test]
    fn withdraw_moves_free_then_oldest_lots_with_their_dates() {
        let mut h = ledger();
        h.free_qty = 3;
        h.append(5, 10 * DAY, 7 * DAY, LotSource::AuctionSettle, false, 1)
            .unwrap(); // newer
        h.append(4, 2 * DAY, 7 * DAY, LotSource::PrimaryMint, true, 3)
            .unwrap(); // older
        let (free, lots) = h.withdraw(9).unwrap();
        assert_eq!(free, 3);
        assert_eq!(lots.len(), 2);
        assert_eq!(
            (lots[0].qty, lots[0].acquired_ts, lots[0].restriction_expiry),
            (4, 2 * DAY, 9 * DAY),
            "oldest first, dates kept"
        );
        assert!(lots[0].affiliate && lots[0].exemption_path == 3);
        assert_eq!(
            (lots[1].qty, lots[1].acquired_ts, lots[1].source),
            (2, 10 * DAY, LotSource::AuctionSettle),
            "split: 2 of the newer lot's 5"
        );
        assert_eq!((h.free_qty, h.lot_count, h.total().unwrap()), (0, 1, 3));
        assert_eq!(h.live()[0].qty, 3, "remainder keeps its lot");
        assert!(h.withdraw(4).is_err(), "only 3 left");
        assert!(h.withdraw(0).is_err());
    }

    /// The receiver gets the same seasoning: unseasoned stays unseasoned,
    /// FREE stays sellable (pledge tacking, both directions).
    #[test]
    fn deposit_keeps_the_seasoning_it_receives() {
        let mut from = ledger();
        from.free_qty = 2;
        from.append(6, 20 * DAY, 7 * DAY, LotSource::PrimaryMint, false, 0)
            .unwrap();
        let mut to = ledger();
        let (free, lots) = from.withdraw(8).unwrap();
        to.deposit(free, &lots, 21 * DAY).unwrap();
        assert_eq!(to.total().unwrap(), 8);
        assert_eq!(to.sellable(21 * DAY).unwrap(), 2, "the lot still seasons");
        assert_eq!(to.sellable(27 * DAY).unwrap(), 8, "from its original date");
        assert_eq!(from.total().unwrap(), 0);
        // And back again, unchanged.
        let (free, lots) = to.withdraw(8).unwrap();
        from.deposit(free, &lots, 22 * DAY).unwrap();
        assert_eq!(from.sellable(26 * DAY).unwrap(), 2);
        assert_eq!(from.sellable(27 * DAY).unwrap(), 8);
    }
}
