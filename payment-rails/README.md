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

## Status: R4 crank + ledger

| Piece | Phase | State |
|---|---|---|
| `RailsConfig` (`["rails_config"]`): authority, usdc_mint, fee_receiver, reserve, operator, **registrar**, fee_bps, reserve_bps, paused, depeg_breaker | R0–R3 | ✅ |
| `initialize_config`, gated to the program's **upgrade authority** | R0 | ✅ |
| `set_operator`, `set_breaker`, `set_fees` (rails hard caps: fee ≤ 200 bps, reserve ≤ 200 bps; these are not the doc 06 §11 prudential floor, see `credit_math::prudential`), **`set_registrar`**, authority-only and evented | R1–R3 | ✅ |
| `CashSession` (`["session", operator, id]`): records at open the deadlines, bond, cap, **asset mint and decimals, fee receiver, fee bps, ROFR authority**, none of which can change later | R1/R2 | ✅ |
| `open_session`, `mark_revealed`, `fail_session`, **`finish_session`** (operator only; none of them move value) | R1/R2 | ✅ |
| Bid escrow: `deposit` (cash + bond) · **ask escrow: `open_asset_vault` → `deposit_ask`** (asset + USDC bond; separate asset vault the gate can thaw first) | R1/R2 | ✅ |
| `forfeit_bond` (either side) · `refund` (bids) · **`refund_ask`** (asks: asset + bond). Anyone can call them, and they're never pausable | R1/R2 | ✅ |
| **`settle_fill`**: atomic DvP, one fill per instruction. A fill-receipt PDA `["fill", session, index]` makes a replayed fill fail | R2 | ✅ |
| **`settle_rofr_fill`**: the issuer pays cash from its own account and takes the seller's escrowed asset (AE-P7) | R2 | ✅ |
| **Payout registry** `PayoutDestination` (`["payout", owner]`): `register_destination`, `renew_destination`, `revoke_destination` (registrar only; attestation hash + expiry, no PII) | R3 | ✅ |
| **Facility** `Facility` + vault, `Advance` (`["advance", facility, borrower]`): `open_facility`, `fund_facility`, `advance_funds`, `repay` | R3 | ✅ |
| **`sweep_proceeds`** (QC-P4): claim-sale proceeds repay the facility first; the remainder goes to the holder | R3 | ✅ |
| **`distribute_coupon`** (TE-P4): one period down the `credit-math` waterfall; one receipt per period | R3 | ✅ |
| **Crank** ([`crank/`](crank/README.md), TypeScript, `@solana/kit`): idempotent settle / refund / finish, priority fees, retries on transient errors only | R4 | ✅ |
| **Reconciliation ledger**: escrow balances == open obligations; facility vault identity | R4 | ✅ |
| **Evidence export**: decoded cash events as JSONL + a SHA-256 manifest | R4 | ✅ |
| **Kill-the-crank drill** (`npm run drill`, real validator): refunds complete via any participant | R4 | ✅ |

### How a fill settles

For a fill of `qty` asset base units at `price` (micro-USDC per whole unit),
the program computes, using `credit-math`:

```
gross      = ceil(qty × price / 10^asset_decimals)   ← buyer's escrow pays (rounds against the buyer)
fee        = floor(gross × fee_bps / 10⁴)            → fee_receiver (recorded at session open)
to_reserve = floor(gross × reserve_bps / 10⁴)        → reserve (recorded at session open)
to_seller  = gross − fee − to_reserve                → seller's own USDC account
qty                                                  → buyer's own asset account (same instruction)
```

`to_seller + fee + to_reserve == gross` always holds. That's checked by a
360-combination sweep in the unit tests (not exhaustive over `u64`) and
against real balances in LiteSVM. The
buyer's unfilled cash and bond come back through `refund` after
`finish_session` (or at the hard deadline). Unsold asset comes back through
`refund_ask`.

**What the operator can and can't do.** The operator picks the matching
(buyer, seller, qty, price) and is trusted to apply the auction's clearing
correctly. It **can't** send value anywhere else:
- the seller is paid only into their own account, and the buyer receives only into theirs;
- fees and the reserve go only to the accounts recorded at session open;
- a seller is paid only by delivering asset that's already escrowed, and a buyer pays only from their own escrow, capped at what they deposited.

These rules are pinned down by `operator_cannot_redirect_any_leg`.

### Payout router (R3): "private" payouts

Every R3 payout to a person goes **only** to a wallet in the payout
registry. That covers facility advances, the holder's share of a sweep, and
senior and junior coupons.

- **Registry.** A `PayoutDestination` holds `{owner, token_account,
  attestation_ref_hash, expires_at, active}`. Only the **registrar** writes it;
  the registrar stands in for the SAS attester until the gate's attestation
  read is on-chain. The payout account must be the owner's own USDC account.
  Nothing but a 32-byte hash of the off-chain attestation is stored.
- **Typed rejections.** A payout checks the destination by hand instead of
  through an automatic anchor check, so that *every* way of not being
  allowlisted fails with a typed reason:

  | Situation | Error |
  |---|---|
  | never registered · not a registry account · another wallet's entry · a token account other than the registered one | `DestinationNotAllowlisted` |
  | revoked (checked first; revocation takes precedence) | `DestinationRevoked` |
  | attestation expired | `DestinationExpired` |

- **Facility** (LF-P1/P2/P6, cash side). The curator (the facility engine's
  signer; underwriting stays in that engine) opens a fund with its own vault.
  Anyone can fund it. `advance_funds` pays only a registered destination and
  is blocked while paused or depegged. `repay` accrues interest with
  `credit_math::accrue_interest` (simple, floored), applies payment
  **interest first**, never takes more than is owed, and closes the advance
  once it's repaid.
- **Sweep** (QC-P4). Claim-sale proceeds repay the facility **first**
  (interest, then principal), and the remainder goes to the holder's
  registered destination. `to_facility + to_holder == proceeds` exactly. If
  the holder is revoked, a sweep that would pay them anything is refused
  outright. A sweep that only repays the facility still goes through.
- **Coupons** (TE-P4). `credit_math::distribute_cash` runs fees → senior
  coupon → reserve top-up → senior amortization → junior coupon → junior
  residual. The rails group those lines into fee receiver / reserve / senior
  / junior and **fail unless they sum exactly to the cash in**. Holder lines
  that receive anything must be allowlisted. A receipt per
  `(distributor, period)` makes a replay fail.

### Escrow rules

| When | Unrevealed escrow | Revealed escrow |
|---|---|---|
| Before `commit_end` | deposits accepted; locked | — |
| `commit_end` ≤ now < `reveal_end` | locked | locked |
| `reveal_end` ≤ now < `settle_deadline` (open) | refundable: **cash/asset → owner, bond → reserve** | locked; **fills settle here** |
| Session `Settled` (`finish_session`) | cash/asset → owner, bond → reserve | refundable: **remaining cash/asset + bond → owner** |
| now ≥ `settle_deadline` | cash/asset → owner, bond → reserve | remaining cash/asset + bond → owner |
| Session failed **before** `reveal_end` | everything → owner | everything → owner |
| Session failed **after** `reveal_end` | cash/asset → owner, bond → reserve | everything → owner |

- Refunds and forfeits can be called by **anyone**. Every destination is fixed to the escrow owner's accounts, or to the reserve recorded at open.
- `paused` and `depeg_breaker` block `open_session`, deposits and **settlement**. They **never** block refunds (AE-P10, AC-11).
- Tokens donated into a vault can't block its close. The surplus goes to the owner.
- A vault holding **less** than recorded, for example because NPCS's permanent delegate pulled asset from an ask vault, can't block it either. The refund returns what is held plus the bond, closes everything, and emits `AssetShortfall` / `CashShortfall` (#62). One case remains outside the rails' control: if the asset's **freeze** authority freezes an ask vault, `refund_ask` fails until it is thawed.
- Deposits check that the vault received exactly what was recorded, so a lossy mint is refused.
- The bid stays sealed. The rails only see the cash amount (`qty × limit`), never qty or limit separately.
- **Trust assumption:** the operator reports reveals and chooses the fills (see above). It's meant to be the auction engine's signer PDA (PHASES Q1).

## Layout

```
payment-rails/
├── Anchor.toml              own Anchor workspace (anchor 1.1.2 · solana 2.3.0)
├── Cargo.toml               own Cargo workspace (overflow-checks = true)
├── keys/                    program keypair: gitignored, back it up (§ Keys)
├── programs/payment-rails/src/
│   ├── lib.rs               entrypoints
│   ├── state.rs             config, sessions, escrows, fills, payout registry, facility, coupons
│   ├── error.rs             RailsError (append-only codes)
│   ├── events.rs            event families
│   └── instructions/        initialize_config · admin · session · escrow · settle · payout · facility · coupon · token
├── tests-svm/               LiteSVM integration tests (own workspace + lockfile)
└── crank/                   R4: crank, ledger, evidence export, kill-the-crank drill (TypeScript)
```

This folder is **not** a member of the root workspace. The reasons differ
for its two crates:

- `tests-svm` **must** stay separate. Adding litesvm to the root lockfile
  pulls in the agave tree and downgrades solana crates the root SBF build
  depends on.
- The program crate is kept out **by choice**, for isolation. It is not
  forced by the resolver: adding it to the root `members` only adds crates
  and changes none of the root's existing versions (checked 2026-09 against
  anchor 1.1.2). Consolidating it later is possible. `credit-math` is used read-only via
`path = "../../../programs/credit-math"`. CI covers it with dedicated jobs in
`.github/workflows/ci.yml` (`payment-rails`, `payment-rails/crank`) that run
everything in "Build and test" below from inside this folder, plus an IDL
drift check on `crank/idl/payment_rails.json` (PHASES.md §2).

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
(cd crank && npm install && npm run typecheck && npm test && npm run drill)   # R4, needs the .so
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

Two secrets control this program. Neither is committed, and neither can be
recreated from the repo.

| Secret | Public key | Lives in | If lost |
|---|---|---|---|
| Program keypair | `GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn` | `keys/payment_rails-keypair.json` (gitignored) | The program can't be deployed at this id on any **new** cluster (mainnet included). The live devnet program is unaffected: upgrades are signed by the upgrade authority, not by this key. |
| Upgrade authority | `9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ` | operator wallet (`AUTHORITY=` for `deploy.sh`) | The devnet program is **frozen at its current build forever**: no bug fix, no upgrade. |

The same applies outside this folder: credit-gate's program keypair
(`target/deploy/credit_gate-keypair.json`, `7yREa5V3…Qa9c`) and upgrade
authority (`Bprmmqrz…jKgp`), and the NPCS mint's single admin key
(`2xhjfAdr…6sRXMh`, holding mint, freeze, permanent-delegate and metadata
authority). See the root [`DEPLOYMENTS.md`](../DEPLOYMENTS.md).

### Backup policy

- Keep **at least two offline copies** of each secret above, in different
  places (for example an encrypted USB drive and a password manager's secure
  file store). A copy under `target/` does not count: `cargo clean` deletes it.
- After every backup or restore, check the copy:
  `solana-keygen pubkey <copy>` must print the public key in the table.
- Record **who** holds each copy (not where) in the team's private ops notes,
  never in this repo.

### Recovery

- **Fresh clone, key needed.** Restore from a backup, then check it:
  ```bash
  mkdir -p keys && cp <backup>/payment_rails-keypair.json keys/
  solana-keygen pubkey keys/payment_rails-keypair.json   # → GdWmRivx…Pocn
  mkdir -p target/deploy && cp keys/payment_rails-keypair.json target/deploy/
  ```
  `deploy.sh` stops with these same instructions when the key is missing.
- **Program keypair lost for good.** The devnet program keeps working and
  stays upgradeable. A new cluster needs a new program id: generate a new
  keypair, update `declare_id!`, `Anchor.toml`, `EXPECTED_ID` in `deploy.sh`
  and the crank's IDL, and record the new id in `DEPLOYMENTS.md`.
- **Upgrade authority lost.** There's no recovery. That's the reason to move
  it to a multisig before it matters (next section).
- **A deploy failed partway.** `deploy.sh` lists the buffer(s) still holding
  SOL and prints the `solana program close --buffers` command that reclaims
  them.

### Before mainnet: Squads 2-of-2 upgrade authority

SRS 08 §8 requires every administrative authority to be a Squads 2-of-2. For
this program that's one transaction, signed by the current authority, that
points the upgrade authority at the Squads **vault** PDA (not the multisig
account):

```bash
solana program set-upgrade-authority GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn \
  --upgrade-authority <current-authority.json> \
  --new-upgrade-authority <SQUADS_VAULT_PDA> \
  --skip-new-upgrade-authority-signer-check -u devnet
```

After that, upgrades go through a Squads proposal. Record the change as a new
row in `DEPLOYMENTS.md`.

`credit-gate` is different. Each `MarketConfig` has its own
`gate_config_authority`, and handing the upgrade authority to Squads does not
change it. Move each config with `propose_gate_authority` →
`accept_gate_authority` before the old key is retired (root
[`DEPLOYMENTS.md`](../DEPLOYMENTS.md), #50).

### Verifying without any key

Anyone can check that the live program is the recorded build, with no secret:

```bash
bash crank/scripts/devnet/verify.sh          # CLUSTER=devnet by default
```

It reads the newest row for the cluster in `DEPLOYMENTS.md`, dumps the
on-chain program, and compares its SHA-256 and upgrade authority with the
recorded values. The recorded Build SHA-256 is the verification artifact: the
key isn't needed to show that the deployed bytes are the reviewed bytes.
