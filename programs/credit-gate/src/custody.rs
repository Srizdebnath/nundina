//! CG-P3 custody moves (LF-P5, #202): lots follow pledged units.
//!
//! A pledge moves the borrower's units into a custodian's vault (the
//! liquidity-facility signer PDA). Without these instructions the lots
//! stayed on the borrower's ledger: the borrower kept seasoned quantity it
//! no longer held (enough to sell freshly bought, unseasoned units at once),
//! and the custodian held units with no lots, so a forced sale needed the
//! issuer's key to invent some.
//!
//! - `register_custodian` / `deregister_custodian` (gate-config authority):
//!   which wallets may hold lots for others on this market.
//! - `custody_pledge` (custodian + pledgor sign): the pledgor's lots move,
//!   **with their acquisition dates**, to the pledge's own ledger, kept
//!   under [`pledge_holder`] — a gate address nobody can sign for, one per
//!   pledge, so one borrower's lots never mix with another's.
//! - `custody_release` (custodian signs): lots move out of one of its
//!   pledge ledgers (or its own ledger) to another holder, still with their
//!   dates: back to the borrower on repayment, to the lender on a workout,
//!   or onto the custodian's own ledger before a forced sale (its seller is
//!   the custodian, so `settle_check` debits that ledger).
//! - `custody_sale` (custodian signs): a ladder sale. Seasoned quantity is
//!   debited like any sale (C2); an admitted buyer gets a new lot seasoning
//!   from now; the issuer buying its own security back gets none (as on the
//!   ROFR path).
//!
//! Keeping the dates on a pledge, its release and a workout is the
//! Rule 144(d)(3)(iv) pledgee tacking analog; it is flagged for counsel
//! (doc 07, #150) with the other seasoning questions.

use anchor_lang::prelude::*;

use crate::blocklist::{require_clear, BlockEntry};
use crate::eligibility::HolderLink;
use crate::lots::{HolderLots, LotEvent, LotSource};
use crate::{GateError, MarketConfig};

/// A wallet allowed to hold lots for others on one market (LF-P5).
#[account]
#[derive(InitSpace)]
pub struct Custodian {
    pub mint: Pubkey,
    pub custodian: Pubkey,
    pub bump: u8,
}

impl Custodian {
    /// PDA seed prefix: `["custodian", mint, custodian]`.
    pub const SEED: &'static [u8] = b"custodian";
}

/// Seed prefix of [`pledge_holder`].
pub const PLEDGE_SEED: &[u8] = b"pledge";

/// The holder key a pledge's ledger is kept under:
/// `["pledge", mint, custodian, pledge_ref]` of this program. It is never a
/// signer, so only `custody_release` / `custody_sale`, signed by
/// `custodian`, can take lots back out.
pub fn pledge_holder(mint: &Pubkey, custodian: &Pubkey, pledge_ref: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            PLEDGE_SEED,
            mint.as_ref(),
            custodian.as_ref(),
            pledge_ref.as_ref(),
        ],
        &crate::ID,
    )
    .0
}

/// What a custody move did.
pub mod move_kind {
    pub const PLEDGE: u8 = 0;
    pub const RELEASE: u8 = 1;
    pub const SALE: u8 = 2;
}

/// Emitted by every custody move (CG-P7).
#[event]
#[derive(Debug)]
pub struct LotsMoved {
    pub mint: Pubkey,
    pub custodian: Pubkey,
    /// [`move_kind`].
    pub kind: u8,
    pub from_holder: Pubkey,
    /// The receiving holder; the issuer's key on an issuer custody sale.
    pub to_holder: Pubkey,
    pub qty: u64,
    /// Of `qty`, how much moved as FREE (already seasoned and merged).
    pub free_qty: u64,
    /// Lots moved with their own dates (0 on a sale).
    pub lots: u8,
}

/// Emitted by `register_custodian` / `deregister_custodian`.
#[event]
#[derive(Debug)]
pub struct CustodianSet {
    pub mint: Pubkey,
    pub custodian: Pubkey,
    pub registered: bool,
    pub authority: Pubkey,
}

/// `from` is the custodian's own ledger (`pledge_ref == default`) or the
/// ledger of its pledge `pledge_ref`.
fn require_custody_ledger(
    from: &HolderLots,
    custodian: &Pubkey,
    pledge_ref: &Pubkey,
) -> Result<()> {
    let expected = if *pledge_ref == Pubkey::default() {
        *custodian
    } else {
        pledge_holder(&from.mint, custodian, pledge_ref)
    };
    require_keys_eq!(from.holder, expected, GateError::NotCustodyLedger);
    Ok(())
}

pub fn process_pledge(ctx: Context<CustodyPledge>, pledge_ref: Pubkey, qty: u64) -> Result<()> {
    let a = ctx.accounts;
    let now = Clock::get()?.unix_timestamp;
    let custodian = a.custodian.key();
    let mint = a.mint.key();
    require_keys_eq!(
        a.pledge_lots.holder,
        pledge_holder(&mint, &custodian, &pledge_ref),
        GateError::NotCustodyLedger
    );
    let (free, lots) = a.pledgor_lots.withdraw(qty)?;
    a.pledge_lots.deposit(free, &lots, now)?;
    emit!(LotsMoved {
        mint,
        custodian,
        kind: move_kind::PLEDGE,
        from_holder: a.pledgor_lots.holder,
        to_holder: a.pledge_lots.holder,
        qty,
        free_qty: free,
        lots: lots.len() as u8,
    });
    Ok(())
}

pub fn process_release(ctx: Context<CustodyRelease>, pledge_ref: Pubkey, qty: u64) -> Result<()> {
    let a = ctx.accounts;
    let now = Clock::get()?.unix_timestamp;
    let custodian = a.custodian.key();
    require_custody_ledger(&a.from_lots, &custodian, &pledge_ref)?;
    require_clear(&a.config, &a.to_block)?;
    let (free, lots) = a.from_lots.withdraw(qty)?;
    a.to_lots.deposit(free, &lots, now)?;
    emit!(LotsMoved {
        mint: a.mint.key(),
        custodian,
        kind: move_kind::RELEASE,
        from_holder: a.from_lots.holder,
        to_holder: a.to_lots.holder,
        qty,
        free_qty: free,
        lots: lots.len() as u8,
    });
    Ok(())
}

pub fn process_sale(
    ctx: Context<CustodySale>,
    pledge_ref: Pubkey,
    buyer: Pubkey,
    qty: u64,
) -> Result<()> {
    let a = ctx.accounts;
    let now = Clock::get()?.unix_timestamp;
    let custodian = a.custodian.key();
    let mint = a.mint.key();
    require_custody_ledger(&a.from_lots, &custodian, &pledge_ref)?;
    require_clear(&a.config, &a.buyer_block)?;
    // C2: only seasoned quantity can be sold out of custody.
    a.from_lots.debit(qty, now)?;
    match (&mut a.buyer_lots, &a.buyer_link) {
        // The issuer buying its own security back: no lot (ROFR analog).
        (None, None) => {}
        (Some(lots), Some(link)) => {
            require_keys_eq!(lots.holder, buyer, GateError::BuyerNotAdmitted);
            require_keys_eq!(link.owner, buyer, GateError::BuyerNotAdmitted);
            let lot = lots.append(
                qty,
                now,
                a.config.seasoning_seconds,
                LotSource::CustodySale,
                false,
                0,
            )?;
            emit!(LotEvent {
                kind: 0,
                mint,
                owner: buyer,
                lot_id: lot.lot_id,
                qty,
                restriction_expiry: lot.restriction_expiry,
                free_qty: lots.free_qty,
            });
        }
        _ => return err!(GateError::BuyerNotAdmitted),
    }
    emit!(LotsMoved {
        mint,
        custodian,
        kind: move_kind::SALE,
        from_holder: a.from_lots.holder,
        to_holder: buyer,
        qty,
        free_qty: 0,
        lots: 0,
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(custodian: Pubkey)]
pub struct RegisterCustodian<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Box<Account<'info, MarketConfig>>,
    #[account(
        init,
        payer = gate_config_authority,
        space = 8 + Custodian::INIT_SPACE,
        seeds = [Custodian::SEED, mint.key().as_ref(), custodian.as_ref()],
        bump
    )]
    pub record: Account<'info, Custodian>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DeregisterCustodian<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Box<Account<'info, MarketConfig>>,
    #[account(
        mut,
        close = gate_config_authority,
        seeds = [Custodian::SEED, mint.key().as_ref(), record.custodian.as_ref()],
        bump = record.bump
    )]
    pub record: Account<'info, Custodian>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct CustodyPledge<'info> {
    /// The custodian's registration on this market.
    #[account(
        seeds = [Custodian::SEED, mint.key().as_ref(), custodian.key().as_ref()],
        bump = record.bump
    )]
    pub record: Box<Account<'info, Custodian>>,
    pub custodian: Signer<'info>,
    /// The holder pledging; signs, as it signs the token transfer.
    pub pledgor: Signer<'info>,
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), pledgor.key().as_ref()],
        bump = pledgor_lots.bump
    )]
    pub pledgor_lots: Box<Account<'info, HolderLots>>,
    /// The pledge's ledger (`open_holder_lots(pledge_holder(..))` first).
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), pledge_lots.holder.as_ref()],
        bump = pledge_lots.bump
    )]
    pub pledge_lots: Box<Account<'info, HolderLots>>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CustodyRelease<'info> {
    #[account(seeds = [MarketConfig::SEED, mint.key().as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, MarketConfig>>,
    #[account(
        seeds = [Custodian::SEED, mint.key().as_ref(), custodian.key().as_ref()],
        bump = record.bump
    )]
    pub record: Box<Account<'info, Custodian>>,
    pub custodian: Signer<'info>,
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), from_lots.holder.as_ref()],
        bump = from_lots.bump
    )]
    pub from_lots: Box<Account<'info, HolderLots>>,
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), to_lots.holder.as_ref()],
        bump = to_lots.bump
    )]
    pub to_lots: Box<Account<'info, HolderLots>>,
    /// CHECK: the receiver's block entry, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), to_lots.holder.as_ref()], bump)]
    pub to_block: UncheckedAccount<'info>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(pledge_ref: Pubkey, buyer: Pubkey)]
pub struct CustodySale<'info> {
    #[account(seeds = [MarketConfig::SEED, mint.key().as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, MarketConfig>>,
    #[account(
        seeds = [Custodian::SEED, mint.key().as_ref(), custodian.key().as_ref()],
        bump = record.bump
    )]
    pub record: Box<Account<'info, Custodian>>,
    pub custodian: Signer<'info>,
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), from_lots.holder.as_ref()],
        bump = from_lots.bump
    )]
    pub from_lots: Box<Account<'info, HolderLots>>,
    /// CHECK: the buyer's block entry, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), buyer.as_ref()], bump)]
    pub buyer_block: UncheckedAccount<'info>,
    /// The buyer's ledger; omitted (with the link) for the issuer.
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), buyer.as_ref()],
        bump = buyer_lots.bump
    )]
    pub buyer_lots: Option<Box<Account<'info, HolderLots>>>,
    /// The buyer's admission; omitted (with the ledger) for the issuer.
    #[account(
        seeds = [HolderLink::SEED, mint.key().as_ref(), buyer.as_ref()],
        bump = buyer_link.bump
    )]
    pub buyer_link: Option<Box<Account<'info, HolderLink>>>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
}
