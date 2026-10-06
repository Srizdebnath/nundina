//! Event families. Every cash movement and every state change emits exactly
//! one event from these families (a refund that also forfeits a bond emits
//! one per movement); the R4 ledger reconciles escrow balances against them.
//!
//! Privacy rule for every event: wallets and amounts only (both are already
//! public in the token transfer itself). Anything that identifies a person
//! or an off-chain document goes in as a 32-byte hash, never raw.

use anchor_lang::prelude::*;

/// Which governed config field group changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum ConfigChange {
    /// First write of the config (initialize_config).
    Initialized,
    /// Fee receiver or reserve re-pointed.
    Treasury,
    /// Authority handed over.
    Authority,
    /// Session operator re-pointed (appended R1).
    Operator,
    /// Fee / reserve bps changed (appended R2).
    Fees,
    /// Payout registrar re-pointed (appended R3).
    Registrar,
}

/// Emitted on every config write.
#[event]
#[derive(Debug)]
pub struct RailsConfigUpdated {
    /// What changed.
    pub change: ConfigChange,
    /// `RailsConfig::fingerprint()` after the write.
    pub new_config_hash: [u8; 32],
    /// Authority that performed the write.
    pub authority: Pubkey,
}

/// Halt switch that moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum Breaker {
    /// Operational pause.
    Pause,
    /// Settlement-asset depeg breaker (06 §2, F4/M5).
    Depeg,
}

/// Emitted when `paused` or `depeg_breaker` flips (`set_breaker`).
#[event]
#[derive(Debug)]
pub struct BreakerChanged {
    /// Which switch.
    pub breaker: Breaker,
    /// New state (`true` = halted).
    pub engaged: bool,
    /// Hash of the off-chain evidence (price sources, incident note).
    pub evidence_hash: [u8; 32],
    /// Authority that flipped it.
    pub authority: Pubkey,
}

/// Escrow lifecycle step (AE-P3/P4/P10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum EscrowAction {
    /// Buyer cash (`qty × limit`) locked at commit.
    Deposited,
    /// Forfeitable bond locked at commit.
    BondPosted,
    /// Unrevealed ⇒ bond moved to reserve (AE-P4).
    BondForfeited,
    /// Timeout / failed-session refund to the depositor (AE-P10, AC-11).
    Refunded,
    /// Operator reported the escrow's bid revealed (no cash moves; amount 0).
    Revealed,
    /// Seller asset locked at commit (amount in asset base units, R2).
    AssetDeposited,
    /// Unsold seller asset returned (amount in asset base units, R2).
    AssetReturned,
    /// `refund_ask` found less asset in the vault than escrowed, e.g. after
    /// a permanent-delegate transfer. Amount = missing base units. The
    /// refund still completed (#62). Appended: indices above are stable.
    AssetShortfall,
    /// A refund found less USDC in the vault than recorded (settlement-mint
    /// delegate or clawback). Amount = missing micro-USDC. The refund still
    /// completed; the forfeit was paid first, up to what was held (#62).
    CashShortfall,
}

/// Unit of [`EscrowEvent::amount`] (#89). Explicit so a consumer that books
/// `amount` never mixes micro-USDC with asset base units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum AmountUnit {
    /// Micro-USDC (settlement mint, 6 dp).
    MicroUsdc,
    /// Base units of the session's asset (`CashSession::asset_decimals`).
    AssetBase,
}

impl EscrowAction {
    /// The unit this action's `amount` is in.
    pub fn unit(self) -> AmountUnit {
        match self {
            EscrowAction::AssetDeposited
            | EscrowAction::AssetReturned
            | EscrowAction::AssetShortfall => AmountUnit::AssetBase,
            EscrowAction::Deposited
            | EscrowAction::BondPosted
            | EscrowAction::BondForfeited
            | EscrowAction::Refunded
            | EscrowAction::Revealed
            | EscrowAction::CashShortfall => AmountUnit::MicroUsdc,
        }
    }
}

/// Emitted for every escrow/bond movement (R1).
#[event]
#[derive(Debug)]
pub struct EscrowEvent {
    /// Lifecycle step.
    pub action: EscrowAction,
    /// Session (or facility) the escrow belongs to.
    pub session: Pubkey,
    /// Escrow PDA.
    pub escrow: Pubkey,
    /// Depositor wallet.
    pub owner: Pubkey,
    /// Amount moved (or missing, for the shortfall actions), in `unit`.
    pub amount: u64,
    /// `MicroUsdc` or `AssetBase` — always `action.unit()` (#89).
    pub unit: AmountUnit,
    /// Signer that triggered it (any participant for refunds).
    pub caller: Pubkey,
}

/// Emitted once per settled fill — the DvP settlement (AE-P7/P8, R2).
/// `to_seller + fee + to_reserve == gross` always, and `qty` of the asset
/// moved seller → buyer in the same instruction.
#[event]
#[derive(Debug)]
pub struct FillSettled {
    /// Session.
    pub session: Pubkey,
    /// Fill index within the session (receipt PDA seed).
    pub fill_index: u32,
    /// Buyer wallet (issuer on the ROFR path).
    pub buyer: Pubkey,
    /// Seller wallet.
    pub seller: Pubkey,
    /// Asset base units delivered.
    pub qty: u64,
    /// Price, micro-USDC per whole asset unit.
    pub price: u64,
    /// Buyer cash released for this fill.
    pub gross: u64,
    /// Cash to seller.
    pub to_seller: u64,
    /// Protocol fee.
    pub fee: u64,
    /// Reserve cut.
    pub to_reserve: u64,
    /// `true` when the issuer's ROFR took the fill (AE-P7).
    pub rofr: bool,
    /// Disclosure-policy hash (aggregate-only print discipline).
    pub disclosure_hash: [u8; 32],
}

/// Legal basis a fill relies on (doc 07 L3–L5, SRS 08 AE-P8). Wire-stable
/// by index and append-only; the indices are credit-gate's exemption codes.
/// credit-gate's settle-time check picks it for every fill (#133), so a
/// settled fill never carries `NotRecorded`; the variant stays for wire
/// stability and for records made before the check existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum ExemptionPath {
    /// No exemption basis was supplied (fills settled before #133).
    NotRecorded,
    /// Securities Act §4(a)(7) resale (doc 07 L4).
    Section4a7,
    /// §4(a)(1½) resale (doc 07 L5).
    Section4a1Half,
    /// Transfer inside the exemption the issue was sold under (doc 07 L3).
    IssuanceExemption,
}

/// Whether the gate's AE-P8 final re-check (attestation still valid, lot
/// seasoning OK, block-list miss) ran inside this settlement. Append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum GateCheck {
    /// No gate re-check ran (fills settled before #133; never now).
    NotPerformed,
    /// The gate approved the transfer in the same transaction.
    Passed,
}

/// One Rule-144 lot a fill draws from (doc 07 L6/L21, CG-P3), as
/// credit-gate's settle-time check debited it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub struct LotRef {
    /// The lot's id in the seller's `HolderLots` ledger. `u32::MAX` is the
    /// FREE bucket (seasoned lots merged together; its times are 0).
    pub lot_index: u32,
    /// Base units this fill took from the lot.
    pub qty: u64,
    /// When the lot was acquired (unix seconds).
    pub acquired_ts: i64,
    /// When the lot's seasoning ends (unix seconds).
    pub restriction_expiry: i64,
}

/// Upper bound on `TransferInstruction.lot_ids`, equal to credit-gate's
/// `MAX_DRAWS` (the gate folds anything past it into one FREE draw).
/// Enforced by [`TransferInstruction::checked`], the only constructor
/// settle uses (#88).
pub const MAX_LOTS_PER_FILL: usize = 8;

/// The AE-P8 transfer record **schema**, emitted once per settled fill right
/// after `FillSettled` (#61). `FillSettled` carries the economics. This
/// event has the fields a transfer agent (doc 07 L21) and a
/// books-and-records export (L22) need to show the transfer was lawful.
///
/// Every fill now runs credit-gate's settle-time check first (#133, #134):
/// block-list on both wallets, the buyer's SAS attestations re-read, the
/// seller's seasoned lots debited. The record carries what the gate
/// returned: the lots drawn, the exemption basis, and `gate_check =
/// Passed`. The crank's evidence manifest counts a fill as `proven` only
/// when all three are present (#88).
#[event]
#[derive(Debug)]
pub struct TransferInstruction {
    /// Session.
    pub session: Pubkey,
    /// Fill index within the session (joins to `FillSettled`).
    pub fill_index: u32,
    /// Seller wallet (asset leaves).
    pub from: Pubkey,
    /// Buyer wallet (asset arrives; issuer on the ROFR path).
    pub to: Pubkey,
    /// Asset base units transferred.
    pub qty: u64,
    /// Price, micro-USDC per whole asset unit.
    pub price: u64,
    /// Lots the transferred units came from, at most `MAX_LOTS_PER_FILL`.
    pub lot_ids: Vec<LotRef>,
    /// Exemption basis relied on for this resale.
    pub exemption_path: ExemptionPath,
    /// Whether the gate's final re-check ran in this transaction.
    pub gate_check: GateCheck,
    /// Disclosure-policy hash (same value as `FillSettled`).
    pub disclosure_hash: [u8; 32],
}

impl TransferInstruction {
    /// Build the record, rejecting more than [`MAX_LOTS_PER_FILL`] lots
    /// with `TooManyLots` (#88): the fill must be split instead.
    #[allow(clippy::too_many_arguments)]
    pub fn checked(
        session: Pubkey,
        fill_index: u32,
        from: Pubkey,
        to: Pubkey,
        qty: u64,
        price: u64,
        lot_ids: Vec<LotRef>,
        exemption_path: ExemptionPath,
        gate_check: GateCheck,
        disclosure_hash: [u8; 32],
    ) -> Result<Self> {
        require!(
            lot_ids.len() <= MAX_LOTS_PER_FILL,
            crate::error::RailsError::TooManyLots
        );
        Ok(Self {
            session,
            fill_index,
            from,
            to,
            qty,
            price,
            lot_ids,
            exemption_path,
            gate_check,
            disclosure_hash,
        })
    }
}

/// Why cash is being paid out (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum PayoutKind {
    /// Queue-claim sale proceeds to the holder (QC-P3/P4).
    ClaimProceeds,
    /// Facility sweep repaying an advance before the holder (QC-P4).
    FacilityRepayment,
    /// Facility advance out of the fund PDA (LF-P1/P2).
    FacilityAdvance,
    /// Tranche coupon via `credit-math::distribute_cash` (TE-P4).
    Coupon,
}

/// Emitted for every payout to an allowlisted destination (R3).
#[event]
#[derive(Debug)]
pub struct PayoutMade {
    /// Why.
    pub kind: PayoutKind,
    /// Source PDA (escrow, facility fund, distribution vault).
    pub source: Pubkey,
    /// Destination token account (must be in the payout registry).
    pub destination: Pubkey,
    /// Micro-USDC paid.
    pub amount: u64,
    /// Hash of the off-chain reference (claim id, advance id, period).
    pub reference_hash: [u8; 32],
}

/// Cash-session lifecycle step (R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum SessionAction {
    /// Session opened with snapshotted deadlines.
    Opened,
    /// Operator aborted the session; all escrows refundable.
    Failed,
    /// Operator closed settlement; remaining escrow refundable (R2).
    Settled,
}

/// Emitted when a cash session opens or fails (R1).
#[event]
#[derive(Debug)]
pub struct SessionEvent {
    /// Lifecycle step.
    pub action: SessionAction,
    /// Session PDA.
    pub session: Pubkey,
    /// Operator.
    pub operator: Pubkey,
    /// Hard refund deadline (AE-P10).
    pub settle_deadline: i64,
    /// Cluster time of the step.
    pub ts: i64,
}

/// Payout-registry step (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum DestinationAction {
    /// First admission of a wallet.
    Registered,
    /// Attestation refreshed / token account re-pointed; re-activates.
    Renewed,
    /// Revoked (block-list precedence).
    Revoked,
}

/// Emitted on every payout-registry write (R3). Hash only — no PII.
#[event]
#[derive(Debug)]
pub struct DestinationEvent {
    /// Step.
    pub action: DestinationAction,
    /// Wallet.
    pub owner: Pubkey,
    /// Registered USDC token account.
    pub token_account: Pubkey,
    /// Attestation reference hash.
    pub attestation_ref_hash: [u8; 32],
    /// Expiry.
    pub expires_at: i64,
    /// Registrar that performed it.
    pub registrar: Pubkey,
}

/// Facility lifecycle step (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum FacilityAction {
    /// Facility + vault created.
    Opened,
    /// Cash added to the vault.
    Funded,
    /// An advance was fully repaid and closed.
    AdvanceClosed,
}

/// Emitted for facility steps that are not payouts (R3).
#[event]
#[derive(Debug)]
pub struct FacilityEvent {
    /// Step.
    pub action: FacilityAction,
    /// Facility PDA.
    pub facility: Pubkey,
    /// Micro-USDC involved (0 for `Opened`).
    pub amount: u64,
    /// Signer.
    pub actor: Pubkey,
}

/// Keeper lifecycle and SLA escalations (doc 06 §8, #127).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum KeeperAction {
    /// Keeper bonded (amount = bond posted).
    Registered,
    /// Bond increased (amount = top-up).
    ToppedUp,
    /// Operator assigned the keeper to a session (amount = 0).
    Assigned,
    /// SLA missed: a third party refunded an escrow the keeper left open and
    /// was paid `amount` from the keeper's bond.
    Escalated,
    /// Assignment released after every escrow closed (amount = 0).
    Released,
    /// Keeper withdrew its whole bond and closed (amount = bond returned).
    Withdrawn,
}

/// Emitted by every keeper instruction (#127).
#[event]
#[derive(Debug)]
pub struct KeeperEvent {
    pub action: KeeperAction,
    /// The keeper's wallet.
    pub keeper: Pubkey,
    /// Session (default when not session-scoped).
    pub session: Pubkey,
    /// micro-USDC, per `action`.
    pub amount: u64,
    /// Who signed.
    pub caller: Pubkey,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lots(n: usize) -> Vec<LotRef> {
        (0..n)
            .map(|i| LotRef {
                lot_index: i as u32,
                qty: 1,
                acquired_ts: 0,
                restriction_expiry: 0,
            })
            .collect()
    }

    fn record(n: usize) -> Result<TransferInstruction> {
        TransferInstruction::checked(
            Pubkey::default(),
            0,
            Pubkey::default(),
            Pubkey::default(),
            1,
            1,
            lots(n),
            ExemptionPath::NotRecorded,
            GateCheck::NotPerformed,
            [0; 32],
        )
    }

    #[test]
    fn lot_cap_is_enforced_at_the_only_constructor() {
        assert!(record(0).is_ok());
        assert_eq!(record(MAX_LOTS_PER_FILL).unwrap().lot_ids.len(), 8);
        assert_eq!(
            record(MAX_LOTS_PER_FILL + 1).unwrap_err(),
            error!(crate::error::RailsError::TooManyLots)
        );
    }

    #[test]
    fn every_escrow_action_has_one_unit() {
        use EscrowAction::*;
        let asset = [AssetDeposited, AssetReturned, AssetShortfall];
        let usdc = [
            Deposited,
            BondPosted,
            BondForfeited,
            Refunded,
            Revealed,
            CashShortfall,
        ];
        assert!(asset.iter().all(|a| a.unit() == AmountUnit::AssetBase));
        assert!(usdc.iter().all(|a| a.unit() == AmountUnit::MicroUsdc));
    }
}

/// #136: an engine grant was written (roles 0 = revoked).
#[event]
pub struct EngineGrantChanged {
    pub engine: Pubkey,
    pub roles: u8,
    pub authority: Pubkey,
}
