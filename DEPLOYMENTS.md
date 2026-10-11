# Official deployments

Registry of official NUNDINA program deployments (G14: published from day one).
Every row is verifiable independently: program ID on-cluster, dump-hash against
the recorded build hash, upgrade authority against the authority map (08 §8).

**Two registries, one rule.** This file lists the *current* official
deployment of every program. `payment-rails` keeps its full per-deploy
history, with genesis hashes, in
[`payment-rails/DEPLOYMENTS.md`](payment-rails/DEPLOYMENTS.md), written by its
`deploy.sh`. When they disagree, that file wins, and this row must be updated
to match.

| Program | Cluster | Program ID | Slot | Deploy signature | Build SHA-256 | Upgrade authority |
|---|---|---|---|---|---|---|
| payment-rails (cash layer, R5; upgrade #3 = `main` `74effa7`: `GrantEngine`, no per-trade fee) | devnet | `GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn` | 509621918 | `2MrwkqUcwT1VzNNS2fNonTdwkLZd7DrvhcErZh5Gwfxic3HeS7JDQCLkNPbg51tMZFNr7LNUindg2YCfpBLdaYBW` | `7c84150aa93781e406fa0f1ffca5e86b7363362214d86d2fab6cefd02b5b2787` | `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV`: Squads v4 vault, 2-of-2 multisig `CHiMWD3q1eHZ8sfUCPzQPp9UWDQ7x2K75notiq1B39z7`, 48h timelock (#126; was `9WH1…2uDZ`) |
| credit-gate v0.2.0 (CG-P1..P7: SAS read, validate, governor, block-list, lot ledger, Token ACL hooks, settle_check; #170) | devnet | `5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX` | 507271972 | `wBS5W8CZ3qmyPv9rduw2wEtpaGAHUGqC2HdqLB58xQzrhaSzYr2A6CrMKoRK8okNwvc2BxriY8bxbY4gvbf4nNQ` | `7668a862b60f4d1602aa0dbf8a8c29a6da56658c82a7a9caf6dd27266dfe2270` | `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV`: Squads v4 vault, 2-of-2, 48h timelock (#171; was `9WH1…2uDZ`) |
| mark-engine v0.1.0 (ME-P1..P7: feed, NAV publication, prints, staleness haircuts; #137) | devnet | `79ypVjMMCDbmxYWaXTTgxz9jidjncoJAhHQC6e13x4LH` | 508551724 | `RomDYUFy5KT6EAiqtdVqhuzPt3JT2ZRo1Ttmsct5FBf6hbhAYLzoeCm1hbvPef1aWU9DaQhF8oehiLzWNMkPABf` | `23dbbd22ea2937960bfccbe8ef2a68bba629e72d31127448f3f031f4299a0c0a` | `9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ` (deployer; Squads hand-over pending, as for credit-gate) |
| auction-engine v0.1.0 (AE-P1..P11: sealed-bid sessions, uniform clear, ROFR; #136) | devnet | `Cw6wNpVJUaofUDZewG8sDKZu3nnHga4GaJJvKn7rSod7` | 508552044 | `USqwBa1X4NJoc549FvdcrjemsTZ3G6jaXMoqnrGQTEmYnxkAydg2RFgT23YW1Gbaca9UtkDnpqBRjt3Kk6vZSnh` | `8957e87deb0dfe6390ffd481ad6605060f683b70a5f2e8a398e2c2b74ca283cc` | `9WH1…2uDZ` (as above) |
| queue-claim v0.1.0 (QC-P1..P5: redemption queue claims; #138) | devnet | `2EpAnJkvw6FH8j97k7WRE72E7oyarZpf7t2TYa26red4` | 508552133 | `5AgmEw858PeSYaYu2CaP3TckFRMPnH8ngBMxX1jo425KNTbmytkWyYbSpnadmyUCUzGxKGrhGGoQUSLP1sLZosBe` | `960d3e745ff5ffc8cdac09d23d10ac2e62538eed59e0b144bc0b20d32ba531a7` | `9WH1…2uDZ` (as above) |
| npcs-treasury v0.1.0 (I1 mint-on-deposit / redeem; #148) | devnet | `4BszRuAP4Gp7vdyu9MjE28gV8YTNtJZSRJRMQHdMut1y` | 508779613 | `3p93oRn2iThavs6MpzKZ8qGzQHxLgMDEcGM2bvno7ER2oYN3E7wKF6nT2QxeEr8311BdKrwnEsYXAy7Rb27DhFY7` | `b387d3c1d656bd997b8fb2f4acd8f30396b09b3453da85817296d528e6c78027` | `9WH1…2uDZ` (as above; on-chain dump == local build) |
| tranche-engine v0.1.0 (TE-P1..P7: senior/junior pools, three-book waterfall; #140) | devnet | `CvcaeyuPu7SrFJ91WniJfBhUbQBkb8QQVL9fyK4wcFAS` | 508808253 | `4SDXWe8YSwkdVwpyAdpw3hDQGLWe4kjAu4UZEFRpQynsmZK4iwVGdamV8xowpWLnmLCYdqUMGX5EnBdFvRdtRs5Z` | `445d725b032d52c24caff7199f3615adba5ddeb1c6d34f135a0fa0703e235bde` | `9WH1…2uDZ` (as above; on-chain dump == local build). No pool yet: `create_pool` is per pool id and its coupons need a rails `ROLE_COUPON` grant (after upgrade #3) |
| liquidity-facility v0.1.0 (LF-P1..P6: advances, health, stale-mark guard, liquidation ladder; LF-P5 custody moves; #139, #205) | devnet | `PTNCoDA5WTbAN5RARzUA19Yr3qJYBzwwH63RPFKS7hY` | 509828983 | `3LhShXuBFAq8eLjqTd96YzLd3KMPX1k3nrbD1Jd9CPDHMz4wpKhnWXJct1tZFo7425D7eWRHcLaWFQWakzhFP8qW` | `b9e657c62afd49e4bef41f97976453df7e452d742bbf537e5bd103be8519acf4` | `9WH1…2uDZ` (as above; build of `main` `aba6d5e`, on-chain dump == local build). Deployed 2026-10-11 ahead of the credit-gate custody upgrade: `open_facility` accepts only the upgrade authority, and NPCS pledges are refused by the live gate until that upgrade lands and the facility signer is registered as a custodian. No facility opened yet |

**Superseded credit-gate deployment (#146, #170).** The P0 skeleton at
`7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c` (slot 503888002, build
`6e8c5087…0e21`, upgrade authority `Bprm…jKgp`) can never be upgraded: no
one on the team holds `Bprm…` or its seed phrase. Instead of upgrading it,
the current source was deployed on 2026-10-04 at a **new program id** under
new keys (row above). The old id is a dead record: nothing in this repo
references it, and payment-rails `CREDIT_GATE_ID` now names the new id.
Configs created against the old program are not trusted (#26).

## Engines on devnet (2026-10-07)

Built from `main` at `74effa7` (`anchor build`, anchor-cli 1.1.2). The
on-chain bytes were checked against the local build: `solana program dump`
gives the same SHA-256 as the table. Wiring is done by
`payment-rails/crank/scripts/devnet/engines.ts`, signed by the rails
authority `9WH1…2uDZ`. Run it with `--status` to read the state back.

| Account | Address | Created by |
|---|---|---|
| mark-engine feed (NPCS) | `7b39TtCi6jLRFbNGSUU7tydefjWqc3w2jf5btdcQjT2k` | `initialize_feed` + `set_publishers` [`4WmrkRmz…dLRQLKjhw`](https://explorer.solana.com/tx/4WmrkRmzY76cNBF75GMrRFzgHBKrxU7emGhXi3AsfEuK5QmFETMrTqkwAEpvVwqYf99L85yMqiW4FVdnLRQLKjhw?cluster=devnet). Weekly cadence (Warned 8 d, Stale 15 d), public prints, **drills off** |
| NAV publisher | `6DZGcFWxr6VHQrXscQsG55gwNbFv3sCttHkcLPVBpUbT` | the feed's only publisher (key `~/.config/nundina/mark-engine-devnet/publisher.json`, off-repo) |
| auction-engine venue | `B6bnWtdFecb3ByMSi5uuKDhXojgbL3xdfEAspH3vmQ3j` | `initialize_venue` [`NnBAf7md…GrAeYhzx`](https://explorer.solana.com/tx/NnBAf7mdCiNFyKHqUPbXDybqEjFkCfxBeKvLkTkVcb7NZjSPgwyjU8v1xvMYj1RhHNUiVoyxJJhKqB2GrQeYhzx?cluster=devnet). No forced-sale opener |
| auction-engine print PDA = feed print source | `AyfBCCp2wDC4pjen8kdZfD7gtB5r4tigW8QHzMRNEMwr` | `set_print_source` [`27oCRh8R…DkzMQC6X`](https://explorer.solana.com/tx/27oCRh8RkiVieC2jKwGDJQKyqNMFTB73BjWBBPB66V1FoZmRBLHqibRbLMExSn2JXoYttWE2deEhNg4PDkzMQC6X?cluster=devnet) |
| queue-claim queue (NPCS) | `8HFKR5FkxMYcCgYqgvvPaPErQMsR4ccXjbHFkDyYavAK` | `init_queue` [`3bJA9w4z…3UMg9dRLoi`](https://explorer.solana.com/tx/3bJA9w4zRaPXyCmY8gzMFmGsyLUrez2r5xGZEdhUDdfbUxZewNa23akkFeTSv65RgPptjr32XKtYoH3UMg9dRLoi?cluster=devnet). Attester = `9WH1…2uDZ` (devnet stand-in for the transfer agent) |

**NAV record (AC-2).** Publication #1: NAV 1.000000 USDC per NPCS (sleeve
`B78W…NKAz` holds 1 USDC, supply 1.000000), `inputs_hash`
`464e3f16…d44f4118`, tx
[`4Jf5dTHT…UGzUXJ`](https://explorer.solana.com/tx/4Jf5dTHT57nFSEooXcQ3xuk8RCBGetxnA7RxLdGdAMSN1v8TqupBdRWKwoaaykbJX4a6fR3wkVYpkuzXUGUd5ZRQ?cluster=devnet).
The inputs document is in `ops/engines/nav/`; anyone can recompute it.
Published by `services` `npm run svc -- nav` (#141), weekly from here on.

**Rails upgrade #3 executed (2026-10-10, `2MrwkqUc…`).** The live rails
build now has `GrantEngine`; `auction-session.ts` grants the auction
operator PDA `ROLE_SESSIONS` as its step 0. `--preflight` on 2026-10-10
reports one blocker only: H's lot seasoning (sellable from
2026-10-11 07:53 UTC). liquidity-facility was deployed on 2026-10-11 (row
above). On `main` it moves gate lots through credit-gate's custody module
(LF-P5), so it is used only after the credit-gate upgrade that carries it
(buffer `5XBXDGMAwPnoobrypF8tF8CQaM9PmbH2KB5hB77CUdW`, Squads proposal + 48h)
and after its signer PDA is registered as an NPCS custodian.

**First live session: runbook.** `payment-rails/crank/scripts/devnet/auction-session.ts`
runs one sealed-bid auction-engine session over the live NPCS mint end to end
(`grant_engine` if missing → buyers attested, validated, thawed → `open_session`
→ H's sealed ask, X's and Y's sealed bids → reveals → the crank's `clear`,
`allocate`, `settle` per fill (DvP through rails `settle_fill` + credit-gate
`settle_check`), `finish` (print) → refunds, reconciliation, signed session
evidence). `--preflight` lists what still blocks it; on 2026-10-08 that is
exactly two dates: rails upgrade #3 (executable 2026-10-09 20:43 UTC) and H's
lot seasoning (2026-10-11 07:53 UTC). Rehearsed on a local clone of devnet
with #3's buffer build (`bash scripts/devnet/rehearse-npcs.sh auction-session`):
cleared 0.970000 against NAV 1.00 (reserve 0.95), 2 fills settled DvP,
X 0.6 / Y 0.4 NPCS, printed, refunds, reconciliation clean, evidence 41 events
in 15 transactions, transfer proof 2/2. That run is the evidence #145 / #172
ask for once it happens on devnet itself.

**NPCS backing → npcs-treasury (#148): hand-over proposed (Squads #5),
executable from 2026-10-13 08:19:53 UTC.** Treasury PDA
`DdbMcM2VAjFaJ5PVgHTaHmHsa1vuwzb9bfduUjRNoijJ`, signer
`Hwp9W1HAzBVBKE3ygiQP4BeYyGkXQ8cEejogYxzShUxk`, treasury USDC vault
`Hho3d6VKpbXY3sN48m76Xed4ivHvrn8aWkQp9CV9CfEL`; until execution the mint
authority stays with the Squads vault and the sleeve holds the backing. npcs-treasury's `initialize` makes its caller the treasury
authority once the mint authority is its signer PDA, so the hand-over is one
Squads vault transaction (`ops/squads` `propose-treasury`): the NPCS mint
authority vault → treasury signer PDA, `initialize` with the vault as
authority, the 1.000000 USDC backing sleeve `B78W…NKAz` → the treasury vault
(I1 holds from the first instant), and the metadata `status` updated to say
the backing is enforced. Tested end to end against the real Squads program on
a local validator (`ops/squads/test-localnet.sh`: an under-funded sleeve is
refused; after execution `deposit` mints 1:1 and `verify` checks the mint
authority, the treasury authority and I1). Proposed on devnet on 2026-10-11 (#5) with:

```bash
cd ops/squads && CLUSTER=devnet PAYER=~/.config/solana/turbine-devnet.json KEYS_DIR=~/.config/nundina/squads-devnet \
  npx tsx migrate.ts propose-treasury 6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4 --sleeve B78WTdsY4MYhAvT757xtdb6KZ61pkBTPzKCtohK3NKAz
# 48 h later:
CLUSTER=devnet PAYER=… KEYS_DIR=… npx tsx migrate.ts execute-treasury
```

After execution the weekly NAV reads the treasury vault instead of the
sleeve (`services` nav-publisher `--source treasury:<treasury PDA>`).

**The init instructions were first-caller-wins; on `main` they are gated.**
`initialize_feed`, `initialize_venue` and `init_queue` made whoever called
them first the authority, and `open_facility` / `create_pool` let anyone
open a facility or pool that drives the rails on the engine's single granted
signer PDA. On `main` all five take the program's ProgramData and accept only
its upgrade authority (the Squads vault once the engines move to it), with a
LiteSVM test each that a stranger gets `NotUpgradeAuthority`. The devnet
builds of mark-engine, auction-engine and queue-claim (2026-10-07) predate the
gate; their accounts were created right after each deploy by the upgrade
authority, so nothing is left to claim. tranche-engine (deployed 2026-10-08)
gets the gated build before any pool exists or `ROLE_COUPON` is granted, and
liquidity-facility is deployed only with it.

## NPCS asset (Token-2022 mint)

| Asset | Cluster | Mint | Details |
|---|---|---|---|
| NPCS — NUNDINA Pilot Credit Sleeve (#171) | devnet | `6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4` | 6dp · DefaultAccountState=Frozen · MetadataPointer(self) · PermanentDelegate · TLV metadata incl. on-chain `status` · **freeze authority = Token ACL MintConfig `3AccB1giyy9Jcc9GZXJKKEqLyNHdY1SPcw1jreX9NRDS` → credit-gate** (permissionless thaw + freeze on) · mint, permanent delegate, metadata pointer + update = Squads vault `8wj1EfUy…LCV` · supply 1.000000 against 1 USDC in the vault's sleeve account `B78WTdsY4MYhAvT757xtdb6KZ61pkBTPzKCtohK3NKAz` (I1) · `cd token && CLUSTER=devnet npm run verify` → ALL CHECKS PASSED (§2.1 included) |
| ~~NPCS (superseded)~~ | devnet | `Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i` | Dead record: every authority is `2xhjfAdr…6sRXMh`, whose key and seed phrase are lost, so it can never be minted, wired or handed over. Supply 0. Replaced by the row above (#171) |

## Verification procedure

```bash
solana program show 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX -u devnet
solana program dump 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX /tmp/dump.so -u devnet
sha256sum /tmp/dump.so   # must equal the Build SHA-256 above

# payment-rails (keyless; fails closed on any mismatch):
bash payment-rails/crank/scripts/devnet/verify.sh

# NPCS asset manifest + provenance:
cd token && CLUSTER=devnet npm run verify
```

## Notes

- **The live devnet build is the current source (#170).** Deployed
  2026-10-04 by `9WH1…2uDZ` (first deploy slot 507268500, tx
  `5CgLegBt…e8BPL9`), then upgraded once at slot 507271972 to the build of
  this branch after merging `main` (doc-comment line shifts change embedded
  panic locations, so the hash moved). The dump SHA-256 equals the local
  build (`7668a862…2270`). The gate root was
  seeded in the same session: `initialize_root(EPCf8gbRaQNqyetsnDxrctgaJjwSzSbxDUSyNBxBavSK)`
  (devnet gate governance key, `~/.config/nundina/credit-gate-devnet/`),
  tx `4ZBcLVncsJ7cyf9xAZXeJuUj5661kw4BGdimymZUQPuVs98iV4DfQiBJFbrsvNj2PGCk5BGpjBZftkWUJYDRg1Mc`.
  Next: `wire.ts market` + `acl` on the NPCS mint (#171/#142), then the
  devnet payment-rails upgrade (Squads proposal, 48h) so its fills CPI
  this gate's `settle_check`.
- The program keypair (`target/deploy/credit_gate-keypair.json`, gitignored)
  is the only way to deploy at this fixed ID on a new cluster. Back it up
  before any `cargo clean`. Upgrades of the live program need only the
  upgrade authority, which makes that key the one whose loss is permanent.
  Every key in this file (credit-gate, NPCS admin, payment-rails, plus the
  rails' fee and reserve accounts) is listed, by public key, in
  [`payment-rails/README.md` § Keys](payment-rails/README.md#keys), with what
  losing or leaking it costs. As of 2026-09 **none has a recorded offline
  backup and none is under Squads** (#57); that section's backup policy and
  hand-over commands are the plan, not a record.
- Verifiable-build reproduction (deterministic SBF flags) lands with the P1
  CI pipeline; until then hashes are recorded from this machine's build.
- **Security (#26):** the superseded v0.1.0 build at `7yRE…Qa9c` lets
  *any* signer run `initialize_config` for a mint and become its permanent
  gate-config authority. The deployed v0.2.0 gates it on the gate root
  (#93), seeded at deploy (above). Configs on the old program are ignored.
- **Checking the deployment (#45).** Anyone can run
  `bash programs/credit-gate/scripts/verify.sh`. It verifies the row above
  keylessly (source id, genesis, dump SHA-256, authority, slot) and reports
  whether this checkout's `anchor build` equals the live build. A future
  upgrade: `anchor build`, `solana program deploy target/deploy/credit_gate.so
  --program-id 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX --upgrade-authority
  <current authority>`, then update this row and re-run `verify.sh`. Once
  the authority is the Squads vault, that is a Squads vault transaction
  with the 48h timelock.
- **Gate root (#93):** `initialize_config` is authorized by the singleton
  `GateRoot` PDA (`["gate_root"]`), not by `ProgramData`, so per-mint
  onboarding (CG-P2, G14) keeps working after doc 06 §11 U1 stages the
  program to immutable. The root is seeded once by the upgrade authority
  and can name a Squads vault directly; it moves only by
  `propose_root_authority` + `accept_root_authority`. Moving the program
  upgrade authority does **not** move the root.
- **What the upgrade authority can still do (#93).** Whoever can replace
  the program binary can rewrite any `MarketConfig` (or the root) with a
  build that ignores these checks: no on-chain guard survives an upgrade
  that removes it. Until the program is finalized, the only control is the
  upgrade authority itself — SRS 08 §8's Squads 2-of-2 **with its 48h
  timelock** — which is not in place today (single bootstrap key, #57).
- **Gate authority hand-over (#50):** a `MarketConfig`'s
  `gate_config_authority` starts as the gate root authority that
  initialized it, which on devnet will be the single bootstrap key above,
  not a Squads vault. Moving the *program* upgrade authority or the root to
  Squads does **not** move any existing config's authority. Each config
  moves with its own two-step hand-over: the current authority calls
  `propose_gate_authority(<squads vault>)`, then the vault calls
  `accept_gate_authority` (which refunds the proposal's rent to the vault,
  #96). Do this for every config when the bootstrap key is retired, and
  before the key is destroyed. A config whose authority key is lost cannot
  be governed again.
- **Squads 2-of-2 migration (#126): executed on devnet for every key this
  machine holds.** Tool: [`ops/squads/`](ops/squads/README.md) (`migrate.ts`,
  end-to-end tested against the real Squads v4 program on a local validator).
  Full record with every signature: [`ops/squads/deployment-devnet.json`](ops/squads/deployment-devnet.json);
  `npx tsx migrate.ts verify` re-checks it keylessly.

  | # | Authority | Was | Now | Executed (devnet) |
  |---|---|---|---|---|
  | 0 | Squads v4 multisig, threshold **2-of-2**, timelock **48h**, no config authority (only a 2-of-2 vote changes members) | — | `CHiMWD3q1eHZ8sfUCPzQPp9UWDQ7x2K75notiq1B39z7` · vault `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV` | ✅ 2026-10-03 · `5dyaTmrZ…b1aWH` |
  | 1 | `payment-rails` upgrade authority | `9WH1…2uDZ` | vault | ✅ 2026-10-03 · `519n9Dw2…VstjP9`; `verify.sh` passes against the new row in `payment-rails/DEPLOYMENTS.md` |
  | 2 | Vault answers to 2-of-2 | — | proposal #1 approved by both members | ✅ approvals 2026-10-03; execute opens 2026-10-05 10:00 UTC (timelock) — `npx tsx migrate.ts prove` |
  | 3 | NPCS mint, permanent delegate, metadata pointer, metadata update (new mint `6vt1wHVQ…VKL4`, #171) | `7Udt…5EEbuh` (bootstrap) | vault; **freeze → Token ACL `MintConfig`** (not the vault) | ✅ 2026-10-04 · `38a9LS93…eE8YF` (`move-mint` leaves a Token ACL freeze authority in place). Pending: the MintConfig's own authority (manual Token ACL freeze/thaw) is still `7Udt…` — payment-rails' session vault accounts are born frozen and need it for the #172 DvP; it moves to the vault right after that fill |
  | 4 | `credit-gate` upgrade authority (`5ycY…X6XX`, #170) | `9WH1…2uDZ` | vault | ✅ 2026-10-04 · `47v3X6D8…avnmuCi`; `programs/credit-gate/scripts/verify.sh` passes against the updated row. The gate root (`EPCf…BavSK`) and the NPCS `MarketConfig` authority stay with the devnet governance key: `append_lot` for primary issuance needs it (#50 hand-over when that path is an auction-engine CPI) |
  | 5 | `payment-rails` upgrade to the build that CPIs the new gate (`CREDIT_GATE_ID` = `5ycY…X6XX`, #172) | — | vault-signed BPF `Upgrade` | ✅ 2026-10-07 · `2YDQ1kVN…uoojSRhCMiJW` (proposal #2, buffer `FqAJ…d5U` = build `b7681924…fbec`, approved 2026-10-04; buffer rent refunded to `9WH1`). `payment-rails/crank/scripts/devnet/verify.sh` passes against the new row in `payment-rails/DEPLOYMENTS.md` |
  | 6 | `payment-rails` upgrade to main (`GrantEngine`/`SetEngineRoles` #181, `MAX_FEE_BPS = 0` #185) | — | vault-signed BPF `Upgrade` | ✅ 2026-10-10 · `2MrwkqUcwT1VzNNS2fNonTdwkLZd7DrvhcErZh5Gwfxic3HeS7JDQCLkNPbg51tMZFNr7LNUindg2YCfpBLdaYBW` (slot 509621918): proposal #3, buffer `FwMUAaQKT6sR6cHApjCRM1UHHfAibJ5feY4wCYV1y1GE` = build `7c84150a…5b2787`, ProgramData extended +32056 B first; registry row above |
  | 7 | `credit-gate` upgrade to main (`aba6d5e`: LF-P5 custody module, market compliance tail #211) | — | vault-signed BPF `Upgrade` | 🕒 proposal #4 created + approved by both members 2026-10-11 07:55 UTC (`5hLtg351…1jKsw3Be`); buffer `5XBXDGMAwPnoobrypF8tF8CQaM9PmbH2KB5hB77CUdW` = build `9cb35c85586fef4f9342f6e2524b2cfcc2ae5837e2b956ff0f34ebd571632459` (861800 B), authority = vault; ProgramData extended +187088 B first (`3cdk6QacT9Bx…`, paid by `9WH1`). **Executable from 2026-10-13 07:55:39 UTC:** `CLUSTER=devnet PAYER=… KEYS_DIR=… npx tsx migrate.ts execute-upgrade 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX`, then the credit-gate registry row, `programs/credit-gate/scripts/verify.sh`, and the facility signer's custodian registration |
  | 8 | NPCS mint authority → npcs-treasury signer `Hwp9…hUxk`; `initialize` (authority = vault); 1.000000 USDC sleeve `B78W…NKAz` → treasury vault `Hho3…CfEL`; metadata `status` → backing enforced (#148) | vault | treasury signer PDA | 🕒 proposal #5 created + approved by both members 2026-10-11 08:19 UTC (`3rNU8sow…DETkKzcH`). **Executable from 2026-10-13 08:19:53 UTC:** `CLUSTER=devnet PAYER=… KEYS_DIR=… npx tsx migrate.ts execute-treasury`, then `migrate.ts verify` and the nav-publisher source → `treasury:DdbMcM2VAjFaJ5PVgHTaHmHsa1vuwzb9bfduUjRNoijJ` |

  **Custody caveat (devnet only):** both member keys were generated on one
  machine (`~/.config/nundina/squads-devnet/`, mode 600, outside the repo).
  That proves the mechanism (2 approvals required, 48h timelock, old key
  powerless) but is not 2-of-2 *custody*. For mainnet the members are two
  people's own hardware wallets; `create` takes no member key from the repo.
  The `RailsConfig` roles (`authority`, `operator`, `registrar`) still sit
  on `9WH1…2uDZ`: they are program-level roles, not the upgrade authority.

## SAS attester (devnet, #122)

| Item | Address |
|---|---|
| SAS program | `22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG` |
| Credential `nundina` | `7H1P4RLB9VhsrSHwqtxJJsMC5wMtF9T7HUN6EUovhfxV` (authority `ErrdHFMVEDdY8idFNKrr4TBsfBgsedxFt89otFm2xCc7`) |
| Authorized signer (attester) | `2L8VPRJme9bEtqoBRTG3kR9JkfbT3GF1DBf48vp2ksQC` |
| `nundina.identity.v1` `[U128, String, U8]` | `6ooYMTCt89VLStUxPPWdTq19Ee9E9TaQCnJPxt3abRUL` |
| `nundina.accreditation.v1` `[Bool, U8]` | `FVxSDQnri8zSREZTXjdgdXc2D8KPyNSFWFsUjG3BuWaU` |
| `nundina.redemption_request.v1` `[VecU8, U64, I64, U32]` | `tdbagMb6dFGBTF83onfstpoeJqBMvHvsgV6m1VQniHs` |

Signatures: `ops/gate/deployment-devnet.json`. Keyless check:
`cd ops/gate && npm ci && npm run sas -- verify`. A team-wallet attestation
pair was issued to the operator wallet `9WH1…2uDZ` (registrar step, pilot
policy: team wallets only) and decodes exactly as the gate's CG-P6 parser
expects.
