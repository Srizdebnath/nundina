//! AE-P8 settle-time gate re-check (#133, #134): every fill CPIs credit-gate's
//! `settle_check`, against the real `credit_gate.so` and the real SAS
//! program. A fill settles only if neither wallet is block-listed, the buyer
//! is admitted with attestations still valid, and the seller's seasoned lots
//! cover it; the transfer record then names the lots, the exemption basis
//! and `gate_check = Passed`. Every refusal leaves no receipt and moves
//! nothing.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AnchorDeserialize, Discriminator, InstructionData, ToAccountMetas};
use base64::Engine;
use common::*;
use credit_gate::{GateError, LotSource, SettleChecked, FREE_LOT_ID};
use payment_rails::{ExemptionPath, GateCheck, LotRef, RailsError, TransferInstruction};
use solana_signer::Signer;
use solana_transaction::Transaction;

const UNIT: u64 = 1_000_000;
const PRICE: u64 = 98_123_456;
const CASH: u64 = 300_000_000;

fn gate_err(e: GateError) -> Fail {
    Fail::Code(u32::from(e))
}

/// Every event `T` in `logs` (rails and gate events share the log stream).
fn events<T: AnchorDeserialize + Discriminator>(logs: &[String]) -> Vec<T> {
    logs.iter()
        .filter_map(|l| l.strip_prefix("Program data: "))
        .filter_map(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .filter(|d| d.starts_with(T::DISCRIMINATOR))
        .map(|d| T::deserialize(&mut &d[T::DISCRIMINATOR.len()..]).unwrap())
        .collect()
}

/// One seller (`ask` seasoned inventory, all on offer) and one buyer, both
/// admitted and revealed; clock at `REVEAL_END`.
struct Pair {
    env: Env,
    s: Pubkey,
    seller: Party,
    buyer: Party,
}

fn pair(tp: Tp, ask: u64) -> Pair {
    let mut env = Env::new(tp);
    let s = env.open_session(1);
    let seller = env.party(BOND, ask);
    env.deposit_ask(&s, &seller, ask).unwrap();
    let buyer = env.party(CASH + BOND, 0);
    env.deposit(&s, &buyer.key, &buyer.usdc, CASH).unwrap();
    reveal(&mut env, &s, &[&seller, &buyer]);
    Pair {
        env,
        s,
        seller,
        buyer,
    }
}

fn reveal(env: &mut Env, s: &Pubkey, who: &[&Party]) {
    env.set_time(COMMIT_END);
    for p in who {
        env.mark_revealed(s, &p.key.pubkey()).unwrap();
    }
    env.set_time(REVEAL_END);
}

/// A refused fill: no receipt, no fill counted, nobody's balance moved.
fn assert_nothing_moved(p: &Pair, fill_index: u32, label: &str) {
    assert!(
        !p.env.exists(&receipt_pda(&p.s, fill_index)),
        "{label}: no receipt"
    );
    assert_eq!(p.env.session(&p.s).fills_settled, 0, "{label}");
    assert_eq!(p.env.bal(&p.buyer.asset), 0, "{label}: buyer got nothing");
    assert_eq!(p.env.bal(&p.seller.usdc), 0, "{label}: seller paid nothing");
}

#[test]
fn a_fill_records_the_lots_the_basis_and_a_passed_check() {
    for tp in Tp::ALL {
        let mut p = pair(tp, 2 * UNIT);
        let (a, x) = (p.seller.key.pubkey(), p.buyer.key.pubkey());
        let acquired = T0 - INVENTORY_AGE;
        let f = p.env.fill(&p.s, &p.buyer, &p.seller);
        let meta = p.env.settle(&f, 0, UNIT, PRICE).unwrap();
        assert!(
            meta.compute_units_consumed < 400_000,
            "{tp:?}: {} CU is over the crank's limit",
            meta.compute_units_consumed
        );

        let [rec] = &events::<TransferInstruction>(&meta.logs)[..] else {
            panic!("{tp:?}: one transfer record")
        };
        assert_eq!((rec.from, rec.to, rec.qty), (a, x, UNIT));
        assert_eq!(rec.gate_check, GateCheck::Passed, "{tp:?}");
        assert_eq!(rec.exemption_path, ExemptionPath::Section4a7, "{tp:?}");
        assert_eq!(
            rec.lot_ids,
            vec![LotRef {
                lot_index: 0,
                qty: UNIT,
                acquired_ts: acquired,
                restriction_expiry: acquired + SEASONING,
            }],
            "{tp:?}"
        );
        let [chk] = &events::<SettleChecked>(&meta.logs)[..] else {
            panic!("{tp:?}: one gate check event")
        };
        assert_eq!(
            (chk.seller, chk.buyer, chk.qty, chk.rofr),
            (a, x, UNIT, false)
        );

        // Seller's lot debited, not dropped: UNIT of the 2 left.
        let sl = p.env.lots(&a);
        assert_eq!(
            (sl.lot_count, sl.lots[0].qty, sl.total().unwrap()),
            (1, UNIT, UNIT)
        );
        // Buyer's new lot seasons from the fill.
        let bl = p.env.lots(&x);
        assert_eq!(bl.lot_count, 1, "{tp:?}");
        let lot = bl.lots[0];
        assert_eq!(
            (
                lot.qty,
                lot.acquired_ts,
                lot.restriction_expiry,
                lot.source,
                lot.exemption_path
            ),
            (
                UNIT,
                REVEAL_END,
                REVEAL_END + SEASONING,
                LotSource::AuctionSettle,
                1
            ),
            "{tp:?}"
        );
        assert_eq!(chk.buyer_lot_id, Some(lot.lot_id));
        assert_eq!(p.env.bal(&p.buyer.asset), UNIT, "{tp:?} delivered");

        // The buyer can't sell what it just bought: unseasoned.
        assert_eq!(bl.sellable(REVEAL_END).unwrap(), 0);
    }
}

#[test]
fn a_block_listed_seller_or_buyer_cannot_settle() {
    for blocked_side in ["seller", "buyer"] {
        let mut p = pair(Tp::Spl, UNIT);
        let who = if blocked_side == "seller" {
            p.seller.key.pubkey()
        } else {
            p.buyer.key.pubkey()
        };
        p.env.block(&who);
        let f = p.env.fill(&p.s, &p.buyer, &p.seller);
        assert_eq!(
            p.env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
            gate_err(GateError::Blocklisted),
            "blocked {blocked_side}"
        );
        assert_nothing_moved(&p, 0, blocked_side);
        assert_eq!(
            p.env.lots(&p.seller.key.pubkey()).total().unwrap(),
            UNIT,
            "{blocked_side}: no lot debited"
        );
    }
}

/// C2: only seasoned quantity sells. Inventory acquired inside the
/// seasoning window stays put, whatever the seller holds in total.
#[test]
fn unseasoned_inventory_cannot_be_sold() {
    let mut env = Env::new(Tp::T22);
    let s = env.open_session(1);
    // 0.4 seasoned + 0.6 acquired just now, all 1.0 on offer.
    let seller = env.party(BOND, 2 * UNIT / 5);
    let a = seller.key.pubkey();
    env.raw_asset_mint_to(&seller.asset, 3 * UNIT / 5);
    let now = env.now();
    env.append_lot_at(&a, 3 * UNIT / 5, now);
    env.deposit_ask(&s, &seller, UNIT).unwrap();
    let buyer = env.party(CASH + BOND, 0);
    env.deposit(&s, &buyer.key, &buyer.usdc, CASH).unwrap();
    reveal(&mut env, &s, &[&seller, &buyer]);
    let f = env.fill(&s, &buyer, &seller);

    assert_eq!(
        env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
        gate_err(GateError::SeasoningNotMet),
        "1.0 wanted, 0.4 seasoned"
    );
    assert!(!env.exists(&receipt_pda(&s, 0)));
    env.settle(&f, 0, 2 * UNIT / 5, PRICE)
        .expect("the seasoned 0.4 sells");
    assert_eq!(
        env.settle(&f, 1, 1, PRICE).unwrap_err(),
        gate_err(GateError::SeasoningNotMet),
        "nothing seasoned is left"
    );
    let l = env.lots(&a);
    assert_eq!(
        (l.lot_count, l.lots[0].qty, l.free_qty),
        (1, 3 * UNIT / 5, 0),
        "the young lot is untouched"
    );
}

#[test]
fn a_revoked_or_expiring_buyer_attestation_fails_the_fill() {
    // Revoked: SAS closes the accreditation after admission.
    let mut p = pair(Tp::Spl, UNIT);
    let x = p.buyer.key.pubkey();
    let acc = p.env.accreditation_att(&x);
    p.env.sas_close(acc);
    let f = p.env.fill(&p.s, &p.buyer, &p.seller);
    assert_eq!(
        p.env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
        gate_err(GateError::AttestationInvalid)
    );
    assert_nothing_moved(&p, 0, "revoked");

    // Expiring: valid for the horizon at admission (T0), not at settlement.
    let mut env = Env::new(Tp::Spl);
    let s = env.open_session(1);
    let seller = env.party(BOND, UNIT);
    env.deposit_ask(&s, &seller, UNIT).unwrap();
    let (key, usdc) = env.bidder(CASH + BOND);
    let x = key.pubkey();
    let expiry = REVEAL_END + 86_400 - 1; // horizon is one day
    let (id, ac) = (env.sas.id_schema, env.sas.acc_schema);
    env.sas_attest(id, &x, identity_data(investor_of(&x)), expiry);
    env.sas_attest(ac, &x, vec![1, 1], expiry);
    env.validate(&x).unwrap();
    assert!(env.exists(&env.link_pda(&x)), "admitted at T0");
    let asset = env.asset_account(&x, 0);
    let buyer = Party { key, usdc, asset };
    env.deposit(&s, &buyer.key, &buyer.usdc, CASH).unwrap();
    reveal(&mut env, &s, &[&seller, &buyer]);
    let f = env.fill(&s, &buyer, &seller);
    assert_eq!(
        env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
        gate_err(GateError::AttestationExpired)
    );
}

#[test]
fn a_buyer_the_gate_never_admitted_cannot_receive() {
    let mut env = Env::new(Tp::Spl);
    let s = env.open_session(1);
    let seller = env.party(BOND, UNIT);
    env.deposit_ask(&s, &seller, UNIT).unwrap();
    let (key, usdc) = env.bidder(CASH + BOND);
    let asset = env.asset_account(&key.pubkey(), 0);
    let buyer = Party { key, usdc, asset };
    env.deposit(&s, &buyer.key, &buyer.usdc, CASH).unwrap();
    reveal(&mut env, &s, &[&seller, &buyer]);
    let f = env.fill(&s, &buyer, &seller);
    // No holder link (anchor: AccountNotInitialized).
    assert_eq!(
        env.settle(&f, 0, UNIT, PRICE).unwrap_err(),
        Fail::Code(3012)
    );
    assert!(!env.exists(&receipt_pda(&s, 0)));
}

/// The rails pass the gate accounts unchecked; the gate re-derives every one.
#[test]
fn gate_accounts_cannot_be_swapped() {
    let mut p = pair(Tp::Spl, UNIT);
    let other = p.env.party(0, UNIT); // admitted, with seasoned lots of its own
    let op = p.env.authority.insecure_clone();
    let f = p.env.fill(&p.s, &p.buyer, &p.seller);
    let x = p.buyer.key.pubkey();
    let o = other.key.pubkey();
    let swap = |env: &Env, from: Pubkey, to: Pubkey| -> Instruction {
        let mut ix = env.settle_ix(&op.pubkey(), &f, 0, UNIT, PRICE);
        let m = ix.accounts.iter_mut().find(|m| m.pubkey == from).unwrap();
        m.pubkey = to;
        ix
    };
    // Not credit-gate.
    let ix = swap(&p.env, GATE_ID, credit_gate::SAS_PROGRAM_ID);
    assert_eq!(
        p.env.send(&[ix], &[&op]).unwrap_err(),
        Fail::rails(RailsError::GateProgramMismatch)
    );
    // Another holder's ledger, link or attestation for the buyer's.
    for (label, from, to) in [
        ("lots", p.env.lots_pda(&x), p.env.lots_pda(&o)),
        ("link", p.env.link_pda(&x), p.env.link_pda(&o)),
        ("block entry", p.env.block_entry(&x), p.env.block_entry(&o)),
    ] {
        let ix = swap(&p.env, from, to);
        assert_eq!(
            p.env.send(&[ix], &[&op]).unwrap_err(),
            Fail::Code(2006),
            "{label}: ConstraintSeeds"
        );
    }
    let ix = swap(&p.env, p.env.identity_att(&x), p.env.identity_att(&o));
    assert_eq!(
        p.env.send(&[ix], &[&op]).unwrap_err(),
        gate_err(GateError::AttestationInvalid),
        "someone else's attestation"
    );
    assert_nothing_moved(&p, 0, "swaps");
    p.env
        .settle(&f, 0, UNIT, PRICE)
        .expect("the true accounts settle");
}

/// Nobody but the rails' settlement PDA can drive the gate's ledger writes.
#[test]
fn settle_check_takes_only_the_rails_pda() {
    let mut p = pair(Tp::Spl, UNIT);
    let (a, x) = (p.seller.key.pubkey(), p.buyer.key.pubkey());
    let impostor = p.env.funded();
    let ix = Instruction {
        program_id: GATE_ID,
        accounts: credit_gate::accounts::SettleCheck {
            settlement_authority: impostor.pubkey(),
            config: p.env.gate_config(),
            policy: p.env.gate_policy(),
            seller_block: p.env.block_entry(&a),
            buyer_block: p.env.block_entry(&x),
            seller_lots: p.env.lots_pda(&a),
            buyer_lots: p.env.lots_pda(&x),
            buyer_link: p.env.link_pda(&x),
            identity_attestation: p.env.identity_att(&x),
            accreditation_attestation: p.env.accreditation_att(&x),
            mint: p.env.asset_mint,
        }
        .to_account_metas(None),
        data: credit_gate::instruction::SettleCheck {
            seller: a,
            buyer: x,
            qty: UNIT,
        }
        .data(),
    };
    assert_eq!(
        p.env.send(&[ix], &[&impostor]).unwrap_err(),
        gate_err(GateError::NotSettlementAuthority)
    );
    assert_eq!(
        p.env.lots(&a).total().unwrap(),
        UNIT,
        "seller ledger intact"
    );
}

#[test]
fn rofr_fill_checks_both_wallets_and_debits_the_seller() {
    let mut env = Env::new(Tp::T22);
    let issuer = env.party(1_000_000_000, 0);
    let s = env.open_session_with_rofr(1, issuer.key.pubkey());
    let seller = env.party(BOND, 2 * UNIT);
    let a = seller.key.pubkey();
    env.deposit_ask(&s, &seller, 2 * UNIT).unwrap();
    env.set_time(COMMIT_END);
    env.mark_revealed(&s, &a).unwrap();
    env.set_time(REVEAL_END);

    let meta = env
        .settle_rofr(&issuer, &s, &seller, 0, UNIT, PRICE)
        .unwrap();
    let [rec] = &events::<TransferInstruction>(&meta.logs)[..] else {
        panic!("one transfer record")
    };
    assert_eq!(rec.exemption_path, ExemptionPath::IssuanceExemption);
    assert_eq!(rec.gate_check, GateCheck::Passed);
    assert_eq!(rec.lot_ids.len(), 1);
    assert_eq!(env.lots(&a).total().unwrap(), UNIT, "seller debited");
    assert_eq!(
        env.lots(&issuer.key.pubkey()).lot_count,
        0,
        "the issuer buys back; it gets no lot"
    );

    env.block(&issuer.key.pubkey());
    assert_eq!(
        env.settle_rofr(&issuer, &s, &seller, 1, UNIT, PRICE)
            .unwrap_err(),
        gate_err(GateError::Blocklisted),
        "a blocked issuer can't take delivery either"
    );
}

/// A market that admits non-accredited holders records §4(a)(1½) for
/// their purchases, §4(a)(7) for accredited ones.
#[test]
fn exemption_basis_follows_the_buyers_accreditation() {
    let mut env = Env::new(Tp::Spl);
    env.asset_mint = env.new_mint_in(Tp::T22, 6);
    env.open_gate_market_with(false);
    let s = env.open_session(1);
    let seller = env.party(BOND, 2 * UNIT);
    env.deposit_ask(&s, &seller, 2 * UNIT).unwrap();
    let accredited = env.party(CASH + BOND, 0);
    let (key, usdc) = env.bidder(CASH + BOND);
    env.admit_as(&key.pubkey(), false);
    let asset = env.asset_account(&key.pubkey(), 0);
    let plain = Party { key, usdc, asset };
    for b in [&accredited, &plain] {
        env.deposit(&s, &b.key, &b.usdc, CASH).unwrap();
    }
    reveal(&mut env, &s, &[&seller, &accredited, &plain]);
    for (i, (b, want)) in [
        (&accredited, ExemptionPath::Section4a7),
        (&plain, ExemptionPath::Section4a1Half),
    ]
    .into_iter()
    .enumerate()
    {
        let f = env.fill(&s, b, &seller);
        let meta = env.settle(&f, i as u32, UNIT, PRICE).unwrap();
        assert_eq!(
            events::<TransferInstruction>(&meta.logs)[0].exemption_path,
            want
        );
    }
}

/// Past `MAX_DRAWS - 1` named lots, the rest of a fill comes from FREE as
/// one draw, so a record never outgrows `MAX_LOTS_PER_FILL`.
#[test]
fn a_fill_across_many_lots_stays_within_the_record_cap() {
    let mut env = Env::new(Tp::Spl);
    let s = env.open_session(1);
    let seller = env.party(BOND, 0);
    let a = seller.key.pubkey();
    let now = env.now();
    for i in 0..10 {
        env.append_lot_at(&a, UNIT / 10, now - INVENTORY_AGE + i);
    }
    env.raw_asset_mint_to(&seller.asset, UNIT);
    env.deposit_ask(&s, &seller, UNIT).unwrap();
    let buyer = env.party(CASH + BOND, 0);
    env.deposit(&s, &buyer.key, &buyer.usdc, CASH).unwrap();
    reveal(&mut env, &s, &[&seller, &buyer]);
    let f = env.fill(&s, &buyer, &seller);
    let meta = env.settle(&f, 0, UNIT, PRICE).unwrap();
    let lots = &events::<TransferInstruction>(&meta.logs)[0].lot_ids;
    assert_eq!(lots.len(), payment_rails::MAX_LOTS_PER_FILL);
    assert_eq!(
        lots.iter().map(|l| l.qty).sum::<u64>(),
        UNIT,
        "draws sum to the fill"
    );
    assert_eq!(lots[7].lot_index, FREE_LOT_ID);
    assert_eq!(lots[7].qty, 3 * UNIT / 10);
    assert_eq!(env.lots(&a).total().unwrap(), 0);
}

/// The two programs agree on the wire: ids, the PDA, the discriminators.
#[test]
fn rails_and_gate_agree_on_the_interface() {
    assert_eq!(payment_rails::CREDIT_GATE_ID, credit_gate::ID);
    assert_eq!(credit_gate::PAYMENT_RAILS_ID, payment_rails::ID);
    assert_eq!(credit_gate::SETTLEMENT_AUTHORITY, gate_settle_pda());
    assert_eq!(
        &payment_rails::SETTLE_CHECK_DISCRIMINATOR[..],
        credit_gate::instruction::SettleCheck::DISCRIMINATOR
    );
    assert_eq!(
        &payment_rails::SETTLE_CHECK_ROFR_DISCRIMINATOR[..],
        credit_gate::instruction::SettleCheckRofr::DISCRIMINATOR
    );
    assert_eq!(credit_gate::MAX_DRAWS, payment_rails::MAX_LOTS_PER_FILL);
}

/// 29 accounts: the crank's settle transaction (compute-budget limit +
/// price + the fill) must still fit a legacy transaction, no lookup table.
#[test]
fn a_settle_transaction_fits_without_a_lookup_table() {
    let p = pair(Tp::T22, UNIT);
    let op = p.env.authority.insecure_clone();
    let f = p.env.fill(&p.s, &p.buyer, &p.seller);
    let budget = anchor_lang::prelude::pubkey!("ComputeBudget111111111111111111111111111111");
    let limit = Instruction {
        program_id: budget,
        accounts: vec![],
        data: [vec![2], 400_000u32.to_le_bytes().to_vec()].concat(),
    };
    let price = Instruction {
        program_id: budget,
        accounts: vec![],
        data: [vec![3], 1_000_000u64.to_le_bytes().to_vec()].concat(),
    };
    let ix = p.env.settle_ix(&op.pubkey(), &f, 0, UNIT, PRICE);
    let tx = Transaction::new_signed_with_payer(
        &[limit, price, ix],
        Some(&op.pubkey()),
        &[&op],
        p.env.svm.latest_blockhash(),
    );
    // Legacy wire format: sigs, header, keys, blockhash, instructions
    // (every count here is < 128, so each short-vec length is one byte).
    let m = &tx.message;
    let size = 1
        + 64 * tx.signatures.len()
        + 3
        + 1
        + 32 * m.account_keys.len()
        + 32
        + 1
        + m.instructions
            .iter()
            .map(|i| 1 + 1 + i.accounts.len() + 1 + i.data.len())
            .sum::<usize>();
    assert!(m.account_keys.len() < 128 && m.instructions.iter().all(|i| i.data.len() < 128));
    assert!(size <= 1232, "settle tx is {size} bytes (limit 1232)");
    println!("settle_fill legacy tx: {size} bytes");
}
