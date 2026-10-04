//! AE-P8: the credit-gate re-check inside every fill (#133, #134).
//!
//! Before a fill moves anything, `settle_fill` / `settle_rofr_fill` CPI
//! credit-gate's `settle_check` / `settle_check_rofr`, signed by this
//! program's settlement PDA `["gate_settle"]`, the only key the gate takes
//! those calls from. The gate screens both wallets against the market
//! block-list, re-reads the buyer's SAS attestations, debits the seller's
//! seasoned lots and appends the buyer's lot. Any failure aborts the fill.
//!
//! What the gate hands back (return data) becomes the fill's
//! `TransferInstruction`: the lots drawn and the exemption basis, with
//! `gate_check = Passed`. No fill settles without it, so no fill records
//! `NotRecorded` / `NotPerformed` any more.
//!
//! The gate's PDAs (config, policy, block entries, lot ledgers, holder link)
//! are passed unchecked here: the gate re-derives each from the asset mint
//! and the two escrow owners and rejects anything else.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::{get_return_data, invoke_signed};

use crate::error::RailsError;
use crate::events::{ExemptionPath, LotRef};

/// The credit-gate program (5ycY…X6XX).
pub const CREDIT_GATE_ID: Pubkey = pubkey!("5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX");
/// Seed of the settlement PDA that signs the gate CPI. credit-gate pins
/// the derived address (`SETTLEMENT_AUTHORITY`).
pub const GATE_SETTLE_SEED: &[u8] = b"gate_settle";
/// Anchor discriminator of `settle_check` (`sha256("global:settle_check")`).
pub const SETTLE_CHECK_DISCRIMINATOR: [u8; 8] = [33, 135, 215, 34, 235, 119, 151, 162];
/// Anchor discriminator of `settle_check_rofr`.
pub const SETTLE_CHECK_ROFR_DISCRIMINATOR: [u8; 8] = [154, 195, 89, 189, 222, 163, 184, 59];

/// credit-gate's `LotDraw`, as it arrives in return data.
#[derive(AnchorDeserialize)]
struct GateLotDraw {
    lot_id: u32,
    qty: u64,
    acquired_ts: i64,
    restriction_expiry: i64,
}

/// credit-gate's `SettleOutcome`.
#[derive(AnchorDeserialize)]
struct GateOutcome {
    draws: Vec<GateLotDraw>,
    exemption_path: u8,
    #[allow(dead_code)]
    buyer_lot_id: Option<u32>,
}

/// A passed gate check, ready for the transfer record.
pub struct GateApproval {
    pub lots: Vec<LotRef>,
    pub exemption_path: ExemptionPath,
}

/// The gate accounts, in `settle_check`'s order after the signer. `true`
/// marks a writable account (the lot ledgers).
pub struct GateAccounts<'a, 'info> {
    pub program: &'a AccountInfo<'info>,
    pub authority: &'a AccountInfo<'info>,
    pub authority_bump: u8,
    pub accounts: Vec<(&'a AccountInfo<'info>, bool)>,
}

/// CPI the gate (`discriminator` picks the instruction) and decode its
/// approval. `Err` if the gate refuses (its error aborts the fill) or if
/// its answer is not a well-formed outcome.
pub fn check(
    gate: GateAccounts,
    discriminator: [u8; 8],
    seller: Pubkey,
    buyer: Pubkey,
    qty: u64,
) -> Result<GateApproval> {
    require_keys_eq!(
        *gate.program.key,
        CREDIT_GATE_ID,
        RailsError::GateProgramMismatch
    );
    let mut metas = vec![AccountMeta::new_readonly(*gate.authority.key, true)];
    let mut infos = vec![gate.authority.clone()];
    for (ai, writable) in &gate.accounts {
        metas.push(if *writable {
            AccountMeta::new(*ai.key, false)
        } else {
            AccountMeta::new_readonly(*ai.key, false)
        });
        infos.push((*ai).clone());
    }
    infos.push(gate.program.clone());
    let mut data = discriminator.to_vec();
    (seller, buyer, qty).serialize(&mut data)?;
    invoke_signed(
        &Instruction {
            program_id: CREDIT_GATE_ID,
            accounts: metas,
            data,
        },
        &infos,
        &[&[GATE_SETTLE_SEED, &[gate.authority_bump]]],
    )?;
    let (from, ret) = get_return_data().ok_or(RailsError::GateOutcomeInvalid)?;
    require_keys_eq!(from, CREDIT_GATE_ID, RailsError::GateOutcomeInvalid);
    let outcome =
        GateOutcome::try_from_slice(&ret).map_err(|_| error!(RailsError::GateOutcomeInvalid))?;
    approval(outcome)
}

fn approval(o: GateOutcome) -> Result<GateApproval> {
    let exemption_path = match o.exemption_path {
        1 => ExemptionPath::Section4a7,
        2 => ExemptionPath::Section4a1Half,
        3 => ExemptionPath::IssuanceExemption,
        _ => return err!(RailsError::GateOutcomeInvalid),
    };
    // A passed check always names where the units came from.
    require!(!o.draws.is_empty(), RailsError::GateOutcomeInvalid);
    let lots = o
        .draws
        .into_iter()
        .map(|d| LotRef {
            lot_index: d.lot_id,
            qty: d.qty,
            acquired_ts: d.acquired_ts,
            restriction_expiry: d.restriction_expiry,
        })
        .collect();
    Ok(GateApproval {
        lots,
        exemption_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The discriminators are checked against credit-gate's own crate in
    // tests-svm (`gate.rs`), which links both programs.

    fn outcome(exemption_path: u8, draws: usize) -> GateOutcome {
        GateOutcome {
            draws: (0..draws)
                .map(|i| GateLotDraw {
                    lot_id: i as u32,
                    qty: 5,
                    acquired_ts: 1,
                    restriction_expiry: 2,
                })
                .collect(),
            exemption_path,
            buyer_lot_id: Some(0),
        }
    }

    #[test]
    fn approval_maps_codes_and_refuses_anything_else() {
        let a = approval(outcome(1, 2)).unwrap();
        assert_eq!(a.exemption_path, ExemptionPath::Section4a7);
        assert_eq!(a.lots.len(), 2);
        assert_eq!((a.lots[1].lot_index, a.lots[1].qty), (1, 5));
        assert_eq!(
            approval(outcome(2, 1)).unwrap().exemption_path,
            ExemptionPath::Section4a1Half
        );
        assert_eq!(
            approval(outcome(3, 1)).unwrap().exemption_path,
            ExemptionPath::IssuanceExemption
        );
        // 0 is NotRecorded: the gate never answers that.
        for bad in [0u8, 4, 255] {
            assert!(approval(outcome(bad, 1)).is_err());
        }
        assert!(approval(outcome(1, 0)).is_err(), "no lots, no approval");
    }
}
