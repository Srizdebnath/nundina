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

## Status: R5 devnet + integration (live on devnet)

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
| **Devnet deploy + end-to-end demo** (commit → settle → payout on devnet; `crank/scripts/devnet/`, recorded in [`DEPLOYMENTS.md`](DEPLOYMENTS.md), keyless `verify.sh`) | R5 | ✅ |

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

> **Fee posture: resolved in code (#147, #59).** doc 07 AE-3 requires *no
> protocol-collected per-trade fee*, so `MAX_FEE_BPS` is **0**: `set_fees`
> refuses any fee, `open_session` refuses a config that still carries one
> (the pre-#147 devnet config's 10 bps must be reset with
> `set_fees(0, reserve_bps)` at the upgrade), and every split's `fee` line
> is 0. The `reserve_bps` cut (≤ 200) stays: it funds the protocol reserve
> that absorbs forfeitures and shortfalls, is never revenue, and is
> disclosed on every `FillSettled`. Reintroducing a fee is a legal decision
> (doc 07 L2), not a parameter change.

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

### Bonded keepers (doc 06 §8, #127)

Refunds are permissionless, so a dead crank never traps cash (AE-P10).
Keepers add accountability on top: a keeper bonds USDC into a vault its
`Keeper` PDA owns (`register_keeper`, ≥ `KeeperPolicy.min_bond`); the
session operator assigns it (`assign_keeper`), snapshotting `due_at =
settle_deadline + sla_seconds` and the per-escrow reward. After `due_at`
anyone may call `refund_escalated` / `refund_ask_escalated`: the same
refund as `refund` / `refund_ask` (one shared body), plus the reward paid
from the keeper's bond to the caller and a recorded miss. The reward is
capped by what the bond holds and never blocks the refund. The bond stays
locked while the keeper has a live assignment (`release_assignment` once
the session has no escrow, then `withdraw_keeper`). Tests:
`tests-svm/tests/keeper.rs` (8, both token programs) and the drill.

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
- A vault holding **less** than recorded, for example because NPCS's permanent delegate pulled asset from an ask vault, can't block it either. The refund returns what is held plus the bond, closes everything, emits `AssetShortfall` / `CashShortfall` (#62), and books the gap on `CashSession.total_cash_shortfall` / `total_asset_shortfall`, which `reconcile` reports as a failed check (#89). Two cases remain with a **freeze** authority, outside the rails' control. A frozen ask **vault** blocks `refund_ask` until it is thawed. A frozen **destination** (e.g. NPCS re-freezing a seller whose attestation lapsed) fails typed with `RefundAssetDestinationFrozen` / `RefundUsdcDestinationFrozen` before anything moves, and the same refund succeeds against any other thawed account of that owner (#90).
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
├── fuzz/                    fuzz targets: settle · clearing · pro-rata · waterfall · refund/bond · facility (own workspace + lockfile, #125)
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
  anchor 1.1.2). Consolidating it later is possible. `credit-math` is used read-only (the rails are its only program consumer today: `ceil_div`, `mul_div`, `accrue_interest`, `distribute_cash`) via
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

### Fuzzing (SRS 08 §11, #125)

`fuzz/` is a cargo-fuzz crate, again its own workspace and lockfile. Each
invariant is a plain `check_*` function in `fuzz/src/lib.rs` that runs the
program's own code (`FillSplit::compute`, `VaultDrain::compute`,
`CashSession::refund_route`, `Advance`, `CouponSplit`) and `credit-math`
(`allocate_pro_rata`, `distribute_cash`, `apply_loss`):

| Target | Laws |
|---|---|
| `settle_fill` | fill legs sum to gross; gross is the exact ceiling; fee and reserve are exact floors; Ok exactly when the inputs are valid |
| `clearing` | price ≥ reserve and is a revealed limit; max volume, then min imbalance, nearest reference mark, lowest price (brute-force oracle) |
| `pro_rata` | sums to total; nobody gets more than committed; within one unit of exact; residue by (qty desc, bidder_id asc); deterministic |
| `waterfall` | exact sum; strict priority; loss hits junior → reserve → senior, exactly |
| `refund_bond` | the vault drains exactly, forfeit first; a revealed bond is never forfeit; refunds open by the hard deadline and never re-lock |
| `facility` | H1 50% ceiling; accrual never runs backwards; repayment is interest first and never above what is owed |

```bash
cd fuzz
cargo test --release                                   # stable: 20k seeded cases per target (CI)
cargo +nightly fuzz run settle_fill -- -max_total_time=60   # libFuzzer (CI job `fuzz`)
```

The smoke test fails if fewer than a third of a target's cases reach the
Ok path, so the harness can't silently fuzz only rejections. Each law was
checked against a planted bug (e.g. forfeiting a revealed bond, residue to
the smallest bid, principal before interest) before it was committed.

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

Every privileged key in the repo is listed below, by **public key only**.
No secret is committed, none can be recreated from the repo, and **none of
them is backed up yet in a way this repo can show**. The backup policy below
is the plan, not a record that it was executed (#57). Status: ⚠️ single key,
no recorded backup, for every row.

| Secret | Public key | Lives in | If lost |
|---|---|---|---|
| Program keypair | `GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn` | `keys/payment_rails-keypair.json` (gitignored) | The program can't be deployed at this id on any **new** cluster (mainnet included). The live devnet program is unaffected: upgrades are signed by the upgrade authority, not by this key. |
| Upgrade authority | `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV`: Squads v4 vault of the 2-of-2 multisig `CHiMWD3q1eHZ8sfUCPzQPp9UWDQ7x2K75notiq1B39z7` (48h timelock), since 2026-10-03 (#126; was `9WH1…2uDZ`) | the two member keys (`~/.config/nundina/squads-devnet/`; one machine on devnet, see root `DEPLOYMENTS.md`) | Losing **one** member blocks every upgrade (2-of-2). An upgrade is a Squads proposal, two approvals and a 48h wait; `deploy.sh` alone can no longer upgrade. |
| Former upgrade key `9WH1…2uDZ`, in its remaining roles | `RailsConfig.authority`, `.operator` and `.registrar` (the registrar is initialised to the authority, `initialize_config.rs`) | same wallet | Losing it also loses fee/breaker/registrar governance and session operation. |
| Fee receiver and reserve **token accounts** | `BCppgtLV86NShmezwdUsEAMxCy6BDfpbqpQfpDbqfcsc` (fee) · `2sZdQ5F3UHbehXYxkv67t5JGRxVb3hGvU6gDryiuWNz6` (reserve), both **owned by `9WH1…2uDZ`** (checked on devnet) | created by `setup.ts` from the authority wallet | The reserve receives every **forfeited client bid bond**, so that money sits in a single-key wallet. There is no setter: both are fixed at `initialize_config` (doc 07 L12's non-custodial posture is not met; see #59). |
| credit-gate upgrade authority | Squads vault `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV` (2-of-2, 48h; #171, was the deployer `9WH1…2uDZ`) | member keys per root `DEPLOYMENTS.md` § Squads | Can replace the gate binary, i.e. rewrite any eligibility rule. The old `BprmmqrzBADdQ2iKARtqcDndWV5k22aUGrkMKeySjKgp` (old program `7yRE…Qa9c`) is lost; that program is superseded. |
| credit-gate root / governance (#93, #170) | `EPCf8gbRaQNqyetsnDxrctgaJjwSzSbxDUSyNBxBavSK` | `~/.config/nundina/credit-gate-devnet/governance.json`, outside the repo | The `GateRoot` authority: onboards markets and becomes each new `MarketConfig`'s `gate_config_authority`. Moves to the vault by `propose_root_authority` + `accept_root_authority`. |
| SAS credential authority (#122) | `ErrdHFMVEDdY8idFNKrr4TBsfBgsedxFt89otFm2xCc7` | `~/.config/nundina/sas-devnet/credential-authority.json` on the operator machine, outside the repo | Can change the credential's authorized signers (rotate or add attesters). Theft: add a rogue attester; it is still not trusted by the gate unless the market config also lists it. |
| SAS attester (#122) | `2L8VPRJme9bEtqoBRTG3kR9JkfbT3GF1DBf48vp2ksQC` | `~/.config/nundina/sas-devnet/attester.json`, outside the repo | Issues and closes attestations. Theft: attest anyone eligible for every market listing it; contain by removing it from `MarketConfig.attesters` and the credential. SRS 08 §5 wants it encrypted and never on a dev machine: **not met on devnet**. |
| credit-gate program keypair | `5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX` | `~/.config/nundina/credit-gate-devnet/program.json`, outside the repo | No new-cluster deploys at this id; the live program is unaffected. |
| NPCS bootstrap admin (#171) | `7UdtTMgH4v8YSG8mWX2r8uASgJwAkxyo4h8yb5UEEbuh` | `token/keys/freeze-authority.json` + `~/.config/nundina/npcs-devnet/` on the minting machine (gitignored) | Created mint `6vt1wHVQ…VKL4`; mint, delegate and metadata authority are now the Squads vault. Still the Token ACL MintConfig authority: theft = manual freeze/thaw of any NPCS account until it moves to the vault. The old admin `2xhjfAdrTrWHUiJrTFhGBa22U2zEYriEEr88Kc6sRXMh` (mint `Dy1VW2…dx2i`) is lost; that mint is superseded. |

Verification artifact: the recorded **Build SHA-256** is committed for
every deploy, and `verify.sh` checks it keylessly. The dumped `.so` bytes are
not committed (`*.so` is gitignored); they are published as GitHub release
assets instead, with their SHA-256s:
[`devnet-programs-2026-10`](https://github.com/Srizdebnath/nundina/releases/tag/devnet-programs-2026-10)
holds the live payment-rails and credit-gate devnet builds (#29 fix #4, #99).
That is the recovery path that does not depend on a key. Publish a new
release after every deploy.

### Backup policy

- Keep **at least two offline copies** of each secret above, in different
  places (for example an encrypted USB drive and a password manager's secure
  file store). A copy under `target/` does not count: `cargo clean` deletes it.
- After every backup or restore, check the copy:
  `solana-keygen pubkey <copy>` must print the public key in the table.
- Record **who** holds each copy (not where) in the team's private ops notes,
  never in this repo. That is also where `deploy.sh` sends an operator who
  needs a key restored; this README says *what* to back up, never *where*.

### Recovery

- **Fresh clone, key needed.** Restore from a backup, then check it:
  ```bash
  mkdir -p keys && cp <backup>/payment_rails-keypair.json keys/
  solana-keygen pubkey keys/payment_rails-keypair.json   # → GdWmRivx…Pocn
  mkdir -p target/deploy && cp keys/payment_rails-keypair.json target/deploy/
  ```
  `deploy.sh` needs it only for a **first** deploy at this id, and stops with
  these instructions if it is missing then.
- **Program keypair lost for good.** The devnet program keeps working and
  stays upgradeable: `deploy.sh` upgrades an existing program by its **bare
  program id**, so only the upgrade authority signs (checked on a local
  validator: deploy, delete the keypair, `UPGRADE=1 bash deploy.sh` → `→
  upgrade ✓ verified`). By hand:
  `solana program deploy <so> --program-id GdWmRivx…Pocn --upgrade-authority <auth.json>`.
  A new cluster needs a new program id: generate a new
  keypair, update `declare_id!`, `Anchor.toml`, `EXPECTED_ID` in `deploy.sh`
  and the crank's IDL, and record the new id in `DEPLOYMENTS.md`.
- **Upgrade authority lost.** There's no recovery. That's the reason to move
  it to a multisig before it matters (next section).
- **A deploy failed partway.** `deploy.sh` lists the buffer(s) still holding
  SOL and prints the `solana program close --buffers` command that reclaims
  them.

### Squads 2-of-2 upgrade authority (done on devnet, #126)

SRS 08 §8 requires every administrative authority to be a Squads 2-of-2. On
devnet the upgrade authority is the vault of multisig
`CHiMWD3q1eHZ8sfUCPzQPp9UWDQ7x2K75notiq1B39z7` (48h timelock), moved with
[`ops/squads/migrate.ts`](../ops/squads/README.md). An upgrade is now:
`solana program write-buffer`, `solana program set-buffer-authority <buffer>
--new-buffer-authority <vault>`, then a Squads vault transaction carrying the
loader's `Upgrade` instruction, approved by both members and executed after
the timelock. Record it as a new row in `DEPLOYMENTS.md`; `verify.sh` checks
the authority against the latest row.

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

It reads the newest row for the cluster in `DEPLOYMENTS.md` and fails closed
on each check. The row's program id must equal `declare_id!` in the source.
The row must record a genesis hash, and the endpoint's must match it (a row
without one is rejected, not skipped). The dump's SHA-256, the upgrade
authority and the last-deployed slot must equal the recorded values. RPC
reads are retried with backoff, and an RPC that keeps failing is reported as
an RPC problem, not as a mismatch. The recorded Build SHA-256 is the verification artifact: the
key isn't needed to show that the deployed bytes are the reviewed bytes.
