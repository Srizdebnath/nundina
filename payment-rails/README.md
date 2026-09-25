# payment-rails

The USDC money layer for NUNDINA: escrow, bonds, refunds, the DvP cash leg,
and payouts to attested wallets only. The security (NPCS or a queue-claim)
never leaves Solana and never passes through this program. Only cash moves
here.

- Program: `payment-rails` · id `GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn`
- Plan and phases: [`PHASES.md`](PHASES.md) · Contract with other components: [`INTERFACE.md`](INTERFACE.md)
- SRS: `preresearch/architecture/08-SRS-Pilot-Rails.md` §4.2 (AE-P3/P4/P7/P8/P10), §4.4 (QC-P4), §4.5 (LF-P1/P2/P6)

## What "private" means here

- Payouts go **only** to attested, allowlisted destinations (the payout registry lands in R3).
- **No PII on-chain.** Every off-chain reference (claim id, advance id, disclosure policy, evidence) is a 32-byte hash.
- **No Confidential Transfers.** They're incompatible with the permissioned book (05 D2, 09 §2). Privacy comes from commit–reveal plus aggregate-only prints.

## Status: R0 scaffold

| Piece | State |
|---|---|
| `RailsConfig` PDA (`seeds = ["rails_config"]`): authority, usdc_mint, fee_receiver, reserve, paused, depeg_breaker | ✅ |
| `initialize_config`, gated to the program's **upgrade authority** so the singleton can't be front-run | ✅ |
| `RailsError`: typed, append-only codes, with `credit_math::MathError` mapped 1:1 | ✅ |
| Event families: `RailsConfigUpdated`, `BreakerChanged`, `EscrowEvent`, `FillSettled`, `PayoutMade` | ✅ defined; only `RailsConfigUpdated` is emitted so far |
| Pause/breaker/treasury update instructions | R1–R2 |
| Escrow + bond vaults, permissionless refunds | R1 |
| `settle_fill` cash leg, fill receipts, ROFR cash, depeg check | R2 |
| Payout registry, sweep order, facility, coupons | R3 |

Checks in `RailsConfig` that later instructions rely on:

- **`ensure_accepting()`** rejects when `paused` or `depeg_breaker` is set. Deposit and settle paths call it. **Refund paths never do** (AE-P10: refunds stay open in every state).
- `usdc_mint` must have 6 decimals, because credit-math works in micro-USDC.
- `fee_receiver` and `reserve` must be token accounts of `usdc_mint`, under the same token program, and must be distinct from each other.

## Layout

```
payment-rails/
├── Anchor.toml              own Anchor workspace (anchor 1.1.2 · solana 2.3.0)
├── Cargo.toml               own Cargo workspace (overflow-checks = true)
├── keys/                    program keypair: gitignored, back it up
└── programs/payment-rails/src/
    ├── lib.rs               entrypoints
    ├── state.rs             RailsConfig
    ├── error.rs             RailsError
    ├── events.rs            event families
    └── instructions/        one file per instruction
```

This folder is **not** a member of the root workspace. `credit-math` is used
read-only via `path = "../../../programs/credit-math"`. At merge time (and only
then), the root `Cargo.toml`, `Anchor.toml` and `.github/workflows/ci.yml` gain
this program (PHASES.md §2).

## Build and test

Requirements: Rust stable, anchor-cli **1.1.2** (`avm use 1.1.2`), and
solana-cli **2.3.0** (`agave-install init 2.3.0`).

```bash
cd payment-rails

# One-time: put the program keypair where anchor expects it, so the build keeps
# the fixed id instead of generating a new one.
mkdir -p target/deploy && cp keys/payment_rails-keypair.json target/deploy/

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
anchor build        # → target/deploy/payment_rails.so + target/idl/payment_rails.json
```

`Cargo.lock` was seeded from the root lockfile so both workspaces resolve the
same solana/anchor crate versions.

## Keys

`keys/payment_rails-keypair.json` is the **only** way to deploy at this program
id. It's gitignored (`.gitignore` covers `keys/`, `target/`, `.anchor/` and
`test-ledger/`). Back it up outside the repo before any `cargo clean` or
`rm -rf target`.
