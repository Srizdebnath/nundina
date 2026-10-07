//! Local demo state for the UI (#144) and the off-chain services (#141).
//!
//! Not a test of its own: it runs the same 3-bidder book as `auction.rs`
//! through the real `.so` files, at wall-clock time, and writes every
//! account the programs own to `$UI_FIXTURE_OUT` in `solana-test-validator
//! --account` format. `app/scripts/localnet.sh` loads them next to the same
//! programs, so the UI reads genuine program state:
//!
//! * session 1 — cleared at 0.95 against a 1.00 NAV, both fills settled
//!   DvP through the rails and the gate, print recorded (phase `Printed`);
//! * session 2 — four sealed orders committed, commit window still open
//!   for `$UI_FIXTURE_COMMIT_SECS` (default 600) after the export, so a
//!   wallet can still commit, then `$UI_FIXTURE_REVEAL_SECS` (default 600)
//!   to reveal.
//!
//! Wallet keypairs and the session-2 preimages go to `keys/` and
//! `fixture.json` (local-only throwaway keys; never fund them elsewhere).
//!
//! ```text
//! UI_FIXTURE_OUT=/abs/dir cargo test --test ui_fixture -- --ignored
//! ```

mod common;

use anchor_lang::prelude::Pubkey;
use auction_engine::{Phase, SessionParams, Side};
use common::*;
use solana_keypair::Keypair;
use solana_signer::Signer;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

const UNIT: u64 = 1_000_000;

fn sealed(side: Side, limit: u64, qty: u64, n: u8) -> Sealed {
    Sealed {
        side,
        limit,
        qty,
        nonce: [n; 32],
    }
}

fn params(commit_end: i64, reveal_end: i64, deadline: i64) -> SessionParams {
    SessionParams {
        commit_end,
        reveal_end,
        rofr_window: 0,
        settle_deadline: deadline,
        bond: BOND,
        max_cash: MAX_CASH,
        min_reveals: 3,
        max_discount_bps: 1_000,
        forced_sale: false,
        rofr_authority: Pubkey::default(),
        disclosure_hash: [0xd1; 32],
    }
}

/// A gate-admitted party whose attestations expire a year after `now`
/// (the shared `party` uses a fixed expiry near `T0`, long past on a
/// validator running at wall-clock time).
fn live_party(env: &mut Env, usdc: u64, asset: u64) -> Party {
    let (key, usdc_acct) = env.bidder(usdc);
    let owner = key.pubkey();
    let asset_acct = env.asset_account(&owner, asset);
    let expiry = env.now() + 365 * 86_400;
    let (id, acc) = (env.sas.id_schema, env.sas.acc_schema);
    env.sas_attest(id, &owner, identity_data(investor_of(&owner)), expiry);
    env.sas_attest(acc, &owner, vec![1, 1], expiry);
    env.validate(&owner).expect("validate");
    if asset > 0 {
        let now = env.now();
        env.append_lot_at(&owner, asset, now - INVENTORY_AGE);
    }
    Party {
        key,
        usdc: usdc_acct,
        asset: asset_acct,
    }
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn keypair_json(k: &Keypair) -> String {
    let b = k.to_bytes();
    format!(
        "[{}]",
        b.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
    )
}

#[test]
#[ignore = "writes local demo state; run with UI_FIXTURE_OUT set"]
fn export_ui_fixture() {
    let out = std::path::PathBuf::from(
        std::env::var("UI_FIXTURE_OUT").expect("UI_FIXTURE_OUT=<absolute dir>"),
    );
    let commit_secs: i64 = std::env::var("UI_FIXTURE_COMMIT_SECS")
        .ok()
        .map(|s| s.parse().expect("UI_FIXTURE_COMMIT_SECS"))
        .unwrap_or(600);
    let reveal_secs: i64 = std::env::var("UI_FIXTURE_REVEAL_SECS")
        .ok()
        .map(|s| s.parse().expect("UI_FIXTURE_REVEAL_SECS"))
        .unwrap_or(600);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let mut env = Env::new(Tp::Spl);
    // Everything after the market setup happens in the last hour, so the
    // NAV is Fresh and every attestation is in date on the validator.
    env.set_time(now - 3_600);
    let publisher = env.setup_auction();
    let seller = live_party(&mut env, 2 * BOND, 30 * UNIT);
    let b1 = live_party(&mut env, 40 * UNIT, 0);
    let b2 = live_party(&mut env, 40 * UNIT, 0);
    let b3 = live_party(&mut env, 40 * UNIT, 0);
    let crank = env.funded();

    // Session 1: the auction.rs book, run to the print.
    let t = now - 3_000;
    env.set_time(t);
    env.open_auction(1, params(t + 300, t + 600, now + 86_400))
        .expect("open 1");
    let s1 = [
        sealed(Side::Sell, 900_000, 10 * UNIT, 1),
        sealed(Side::Buy, 970_000, 6 * UNIT, 2),
        sealed(Side::Buy, 950_000, 6 * UNIT, 3),
        sealed(Side::Buy, 930_000, 4 * UNIT, 4),
    ];
    env.commit_ask(1, &seller, &s1[0], 10 * UNIT).expect("ask");
    env.commit_bid(1, &b1, &s1[1], 10 * UNIT).expect("b1");
    env.commit_bid(1, &b2, &s1[2], 10 * UNIT).expect("b2");
    env.commit_bid(1, &b3, &s1[3], 10 * UNIT).expect("b3");
    env.set_time(t + 300);
    for (p, o) in [
        (&seller, &s1[0]),
        (&b1, &s1[1]),
        (&b2, &s1[2]),
        (&b3, &s1[3]),
    ] {
        env.reveal(1, p, o).expect("reveal");
    }
    env.set_time(t + 600);
    env.clear_auction(1, &crank).expect("clear");
    env.allocate(1, &crank, 8).expect("allocate");
    env.settle_auction(1, &crank, 0, &b1, &seller)
        .expect("fill 0");
    env.settle_auction(1, &crank, 1, &b2, &seller)
        .expect("fill 1");
    env.finish_auction(1, &crank).expect("finish");
    let done = env.auction(1);
    assert_eq!(done.phase, Phase::Printed);
    assert_eq!(done.clear_price, 950_000);

    // Session 2: committed, waiting for more bids and then reveals.
    env.set_time(now - 60);
    let commit_end = now + commit_secs;
    let reveal_end = commit_end + reveal_secs;
    env.open_auction(2, params(commit_end, reveal_end, reveal_end + 3_600))
        .expect("open 2");
    let s2 = [
        (&seller, sealed(Side::Sell, 920_000, 8 * UNIT, 11), 8 * UNIT),
        (&b1, sealed(Side::Buy, 960_000, 5 * UNIT, 12), 6 * UNIT),
        (&b2, sealed(Side::Buy, 945_000, 4 * UNIT, 13), 5 * UNIT),
        (&b3, sealed(Side::Buy, 925_000, 3 * UNIT, 14), 4 * UNIT),
    ];
    for (p, o, escrow) in &s2 {
        match o.side {
            Side::Sell => env.commit_ask(2, p, o, *escrow),
            Side::Buy => env.commit_bid(2, p, o, *escrow),
        }
        .expect("commit 2");
    }
    assert_eq!(env.auction(2).phase, Phase::Commit);

    // ── export ──────────────────────────────────────────────────────────
    let accounts_dir = out.join("accounts");
    let keys_dir = out.join("keys");
    std::fs::create_dir_all(&accounts_dir).unwrap();
    std::fs::create_dir_all(&keys_dir).unwrap();
    let owners = [
        payment_rails::ID,
        credit_gate::ID,
        auction_engine::ID,
        mark_engine::ID,
        credit_gate::SAS_PROGRAM_ID,
        Tp::Spl.id(),
        Tp::T22.id(),
        anchor_lang::system_program::ID,
    ];
    let mut n = 0;
    for owner in owners {
        for (addr, a) in env.svm.get_program_accounts(&owner) {
            let json = format!(
                r#"{{"pubkey":"{addr}","account":{{"lamports":{},"data":["{}","base64"],"owner":"{}","executable":false,"rentEpoch":0,"space":{}}}}}"#,
                a.lamports,
                b64(&a.data),
                a.owner,
                a.data.len()
            );
            std::fs::write(accounts_dir.join(format!("{addr}.json")), json).unwrap();
            n += 1;
        }
    }

    let wallets = [
        ("authority", env.authority.insecure_clone()),
        ("publisher", publisher),
        ("seller", seller.key.insecure_clone()),
        ("b1", b1.key.insecure_clone()),
        ("b2", b2.key.insecure_clone()),
        ("b3", b3.key.insecure_clone()),
        ("crank", crank),
    ];
    for (name, k) in &wallets {
        std::fs::write(keys_dir.join(format!("{name}.json")), keypair_json(k)).unwrap();
    }
    let orders: Vec<String> = s2
        .iter()
        .map(|(p, o, escrow)| {
            format!(
                r#"{{"owner":"{}","side":"{}","limit":{},"qty":{},"nonce":"{}","escrowed":{}}}"#,
                p.key.pubkey(),
                if matches!(o.side, Side::Buy) {
                    "Buy"
                } else {
                    "Sell"
                },
                o.limit,
                o.qty,
                hex(&o.nonce),
                escrow
            )
        })
        .collect();
    let wallet_list: Vec<String> = wallets
        .iter()
        .map(|(name, k)| format!(r#""{name}":"{}""#, k.pubkey()))
        .collect();
    let manifest = format!(
        r#"{{
  "exportedAt": {now},
  "rails": "{}",
  "railsConfig": "{}",
  "usdcMint": "{}",
  "assetMint": "{}",
  "venue": "{}",
  "sessions": {{"printed": "{}", "open": "{}"}},
  "commitEnd": {commit_end},
  "revealEnd": {reveal_end},
  "wallets": {{{}}},
  "tokenAccounts": {{"seller": ["{}","{}"], "b1": ["{}","{}"], "b2": ["{}","{}"], "b3": ["{}","{}"]}},
  "openOrders": [{}],
  "accounts": {n}
}}
"#,
        payment_rails::ID,
        env.config,
        env.mint,
        env.asset_mint,
        venue_pda(),
        auction_pda(1),
        auction_pda(2),
        wallet_list.join(","),
        seller.usdc,
        seller.asset,
        b1.usdc,
        b1.asset,
        b2.usdc,
        b2.asset,
        b3.usdc,
        b3.asset,
        orders.join(",")
    );
    std::fs::write(out.join("fixture.json"), manifest).unwrap();
    println!("ui fixture: {n} accounts -> {}", out.display());
}
