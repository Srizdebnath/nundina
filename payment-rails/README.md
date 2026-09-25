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

## Status: R1 escrow vaults

| Piece | Phase | State |
|---|---|---|
| `RailsConfig` PDA (`["rails_config"]`): authority, usdc_mint, fee_receiver, reserve, **operator**, paused, depeg_breaker | R0/R1 | ✅ |
| `initialize_config`, gated to the program's **upgrade authority** so the singleton can't be front-run | R0 | ✅ |
| `set_operator`, `set_breaker` (pause / depeg), authority-only and evented | R1 | ✅ |
| `CashSession` (`["session", operator, id]`): snapshotted deadlines, bond, per-investor cap, mint, reserve | R1 | ✅ |
| `open_session`, `mark_revealed`, `fail_session` (operator only; none of them move cash) | R1 | ✅ |
| `Escrow` (`["escrow", session, owner]`) + its own vault (`["vault", escrow]`, authority = escrow PDA) | R1 | ✅ |
| `deposit` (cash + bond), `forfeit_bond` (permissionless), `refund` (**permissionless, never pausable**) | R1 | ✅ |
| `settle_fill` cash leg, fill receipts, ROFR cash, depeg check at settle | R2 | — |
| Payout registry, sweep order, facility, coupons | R3 | — |

### Escrow rules (what the tests pin down)

| When | Unrevealed escrow | Revealed escrow |
|---|---|---|
| Before `commit_end` | deposits accepted; locked | — |
| `commit_end` ≤ now < `reveal_end` | locked | locked |
| now ≥ `reveal_end` (session open) | refundable: **cash → owner, bond → reserve** | locked (awaiting settlement) |
| now ≥ `settle_deadline` | cash → owner, bond → reserve | refundable: **cash + bond → owner** |
| Session failed **before** `reveal_end` | cash + bond → owner | cash + bond → owner |
| Session failed **after** `reveal_end` | cash → owner, bond → reserve | cash + bond → owner |

- Refunds and forfeits can be called by **anyone**. The cash can only go to a token account owned by the escrow owner, and the bond only to the reserve snapshotted at session open.
- `paused` and `depeg_breaker` block `open_session` and `deposit`. They **never** block `refund` (AE-P10, AC-11).
- Tokens donated into a vault can't block its close. Anything above the recorded amounts goes to the owner.
- A deposit checks that the vault received exactly `cash + bond`, so a lossy mint (for example one with a transfer fee) is refused.
- The bid stays sealed. The rails only see the cash amount (`qty × limit`), never qty or limit separately.
- **Trust assumption:** the operator reports reveals. An operator that withholds `mark_revealed` can make an honest bidder's bond forfeit. The operator is set by the config authority and is meant to be the auction engine's signer PDA (PHASES Q1).

## Layout

```
payment-rails/
├── Anchor.toml              own Anchor workspace (anchor 1.1.2 · solana 2.3.0)
├── Cargo.toml               own Cargo workspace (overflow-checks = true)
├── keys/                    program keypair: gitignored, back it up
├── programs/payment-rails/src/
│   ├── lib.rs               entrypoints
│   ├── state.rs             RailsConfig, CashSession, Escrow + refund rules
│   ├── error.rs             RailsError (append-only codes)
│   ├── events.rs            event families
│   └── instructions/        initialize_config · admin · session · escrow
└── tests-svm/               LiteSVM integration tests (own workspace + lockfile)
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
cargo test --workspace                              # unit tests
anchor build        # → target/deploy/payment_rails.so + target/idl/payment_rails.json
cargo test --manifest-path tests-svm/Cargo.toml     # LiteSVM, needs the .so above
```

`tests-svm/` is a **separate workspace** with its own `Cargo.lock`. litesvm pulls
a large solana runtime tree, and keeping it out of the program's lockfile
stops it from bumping crates the SBF build depends on. Each test runs against
the real `.so`, deployed as an upgradeable program, under **both SPL Token and
Token-2022**.

`anchor build` is the canonical build. To build only the SBF binary (it's
faster because it skips the IDL pass), run
`cargo build-sbf --tools-version v1.52 --manifest-path programs/payment-rails/Cargo.toml --sbf-out-dir target/deploy`.
Without `--tools-version`, solana 2.3.0 defaults to platform-tools v1.48,
whose bundled cargo 1.84 can't parse the edition-2024 crates in the lockfile
(for example `cpufeatures` 0.3.1).

`Cargo.lock` was seeded from the root lockfile so both workspaces resolve the
same solana/anchor crate versions.

## Keys

`keys/payment_rails-keypair.json` is the **only** way to deploy at this program
id. It's gitignored (`.gitignore` covers `keys/`, `target/`, `.anchor/` and
`test-ledger/`). Back it up outside the repo before any `cargo clean` or
`rm -rf target`.
