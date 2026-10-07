# NUNDINA

> **The pricing, queue & risk-transfer engine for gated, NAV-priced private credit — on Solana.**
> Not a venue. Not an AMM. The mechanism that tells a quarterly-marked, gated credit position what it's worth — and turns a denied redemption request into a priceable, financeable claim.

[![Status](https://img.shields.io/badge/status-building_P1-blue)](preresearch/)
[![Phase](https://img.shields.io/badge/P0-open-orange)](#p0-exit-criteria)
[![Token](https://img.shields.io/badge/NPCS-live_on_devnet-8A2BE2)](token/)
[![Docs](https://img.shields.io/badge/docs-10_files_~200KB-success)](preresearch/)
[![License: MIT](https://img.shields.io/badge/license-MIT-lightgrey)](LICENSE)

---

## The one-paragraph version

Tokenized private credit funds (ACRED, STAC, ONyc, SCOPE…) are locked, gated at ~5% per quarter, and NAV-priced only quarterly by the manager itself. In 2026 the gate crisis became the market's defining event — Apollo capped redemptions at 17% of NAV requested vs. a 5% cap; $5B+ sits trapped. NUNDINA is designing and building the mechanism nobody else has built. This paragraph is the **target architecture**; the table below says what exists today: a **sealed-bid uniform-price batch auction** that manufactures an arms-length price where a passive AMM would bleed to adverse selection, a **mark engine** that makes stale appraisals usable, a **queue-claim** that tokenizes denied redemption requests, a **liquidity facility** that can actually liquidate permissioned collateral, a **credit-loss tranching waterfall**, and a **compliance gate** (Token ACL / sRFC37) enforcing per-lot Rule 144 seasoning, holder caps, and jurisdiction policy at account activation.

## Build status — real, on devnet

Everything marked ✅ below is real and independently checkable. Nothing is
mocked, and nothing is claimed that isn't built. The pilot asset is a real,
self-issued Token-2022 mint, but its USDC backing is maintained by process
today, not enforced on-chain ([`token/TOKENOMICS.md` §0](token/TOKENOMICS.md)).

| Component | Status | Where to verify |
|---|---|---|
| **`payment-rails`**: the USDC cash layer. Escrow and bonds (AE-P3/P4), permissionless never-pausable refunds (AE-P10), atomic one-fill DvP (AE-P8), issuer ROFR (AE-P7), facility funding, advances, interest and sweep (LF-P2/P6), exact-conservation cash waterfall (TE-P4) | ✅ **live devnet** · LiteSVM suite + kill-the-crank drill on a real validator. Auction, mark, claim and tranche *semantics* are not built; the rails move their cash | [`payment-rails/`](payment-rails/) · `bash payment-rails/crank/scripts/devnet/verify.sh` |
| `credit-math`: shared fixed-point crate (NAV, clearing, pro-rata, waterfall, PV, prudential floor) | ✅ built · unit + property suites · CI gates 100% line + branch coverage. **Wired:** `payment-rails` uses 4 functions; clearing, pro-rata, NAV, haircut and PV have no program consumer yet | `cargo test --workspace` |
| `credit-gate`: sRFC37 Gate Program | ✅ **on `main` (CG-P1..P7):** SAS attestation read (CG-P6), holder governor (CG-P4), block-list (CG-P5), lot ledger (CG-P3), `validate` + Token ACL `can_thaw`/`can_freeze` hooks (CG-P1), all against the **real** SAS and Token ACL programs in LiteSVM and on a validator. ⚠️ Devnet runs the **pre-#26 build** and NPCS is not yet handed to Token ACL: both need key holders (`ops/gate/README.md`) | `DEPLOYMENTS.md` |
| **NPCS**: the pilot asset (Token-2022, frozen-by-default, TLV metadata) | ✅ **mint live devnet** · manifest verified. Not yet gated or backed on-chain; one bootstrap key holds every authority | [`token/`](token/) · `DEPLOYMENTS.md` |
| Off-chain services (#141) | ✅ built, not hosted: indexer (Postgres, RPC backfill + Helius webhooks), read API, NAV publisher (`credit-math` arithmetic, `publish_nav`), treasury agent (I1 check), crank-runner (phases · refunds · seasoning · distribution), session evidence bundles. Verified on a local validator with the real programs and against devnet (gate history indexed; sleeve BACKED; NAV dry run 1.000000). No Supabase/Helius project provisioned | [`services/`](services/README.md) · `payment-rails/crank/` |
| Next.js 16 app (#144) | ✅ the three demo screens, read live from the cluster: **session** (phase clock, sealed → revealed book, clearing curves vs reserve and NAV, fills settled DvP, print; wallet commit + reveal), **portfolio** (NPCS, lots with seasoning countdown, claims, advances, orders), **gate check** (simulates credit-gate's real `validate` and shows its typed `GateReject` with the inputs it read). Devnet: gate + portfolio show live NPCS state; sessions need the engines deployed (not yet). Full session flow end to end on a local validator with the real programs (`npm run localnet:e2e`). No `/admin`, `/data-room` or allowlist yet | [`app/`](app/README.md) |
| CI (fmt · clippy `-D warnings` · tests · LiteSVM · IDL drift · drill · coverage · app lint + build), every job a required check | ✅ green | `.github/workflows/ci.yml` |
| SAS attester (credential + 3 schemas) | ✅ **live devnet** (#122) | `ops/gate/deployment-devnet.json` · `npm run sas -- verify` |
| Token ACL wiring of NPCS | ✅ devnet (#171): new mint `6vt1wHVQ…VKL4` thaws only through credit-gate; mint/delegate/metadata authority = Squads vault | `ops/gate/`, `token/` |
| mark-engine · auction-engine · queue-claim · facility · tranching engines | scheduled P2–P5 (cash legs already in `payment-rails`) | SRS 08 §12 |

### Programs: as built vs roadmap

Seven programs are named in the design (SRS 08 §4). Five have **no code**:
their names appear only in docs and doc comments (#128). Anything in this
repo that describes them is the target, not a shipped feature.

| Program | Crate | IDL | Tests | Crank | State |
|---|---|---|---|---|---|
| `payment-rails` | ✅ | ✅ | ✅ LiteSVM + drill | ✅ | live devnet |
| `credit-gate` | ✅ | ✅ | ✅ LiteSVM + real SAS/Token ACL | — | CG-P1..P7 on `main`; devnet redeploy pending (#45) |
| `credit-math` (crate) | ✅ | n/a | ✅ unit + property | n/a | built; 4 functions consumed by `payment-rails` |
| `auction-engine` | ✅ | ✅ | ✅ unit + LiteSVM (real rails, gate, SAS, mark) | ⚠️ crankable (permissionless settle/expire), no runner yet (#141) | AE-P1..P11 on `main` (#136); not deployed |
| `mark-engine` | ✅ | ✅ | ✅ unit + LiteSVM | — | ME-P1..P7 on `main` (#137); not deployed; no nav-publisher yet (#141) |
| `queue-claim` | ✅ | ✅ | ✅ LiteSVM: sell path through `auction-engine`, pledge path through `liquidity-facility` | — | QC-P1..P5 on `main` (#138); not deployed |
| `liquidity-facility` | ✅ | ✅ | ✅ unit + LiteSVM (full ladder through auction + rails) | ⚠️ permissionless steps, no runner yet (#141) | LF-P1..P6 on `main` (#139); not deployed |
| `tranche-engine` | ✅ | ✅ | ✅ unit + LiteSVM (waterfall through rails) | ⚠️ permissionless `mark` / `distribute`, no runner yet (#141) | TE-P1..P7 on `main` (#140); not deployed |

Per-AC grades: [SRS 08 §1.1](preresearch/architecture/08-SRS-Pilot-Rails.md#11--as-built-grade-2026-10).

### P0 exit criteria

P0 (#4) was closed with three criteria unmet, and it is **open** again here (#129). Re-baselined in #100: each row
says what is true, and the unmet ones are deferred to P1 explicitly rather
than badged complete.

| # | Criterion (#4) | State | Evidence / deferral |
|---|---|---|---|
| 1 | `services/` (nav-publisher, crank-runner, indexer, evidence-exporter) | ⏭ **deferred to P1** | [`services/README.md`](services/README.md): none built; only `payment-rails/crank` exists (#58) |
| 2 | `credit-math` skeleton with property tests running | ✅ | CI `credit-math — coverage` gates 100% lines + branches |
| 3 | CI: anchor build + test; fmt/clippy; app lint | ✅ | `.github/workflows/ci.yml`: every job is a required check on `main`; `app — lint · typecheck · build` |
| 4 | Squads 2-of-2 authority created; key ceremony + storage runbook | ⚠️ runbook ✅ · **Squads 2-of-2 live on devnet** (48h timelock), `payment-rails` upgrade authority moved ✅ · NPCS and `credit-gate` keys not on the migrating machine, their moves are scripted for the key holders | `ops/squads/`, `DEPLOYMENTS.md` § Squads (#126) |
| 5 | Verifiable builds; program-ID registry | ⚠️ registry ✅ · `anchor build --verifiable` **deferred to P1** | `DEPLOYMENTS.md` + keyless `verify.sh`; pinned toolchains, SHA-256-verified CLI installs and a lockfile-drift check on every SBF build (#98); deployed `.so` files published as release assets (#99). No deterministic (docker) build yet |
| 6 | Helius (dev) + Supabase (dev) provisioned | ⏭ **deferred to P1** | not provisioned; nothing in the repo references them ([`services/README.md`](services/README.md)) |
| 7 | Dependabot `npm` entries | ✅ | `.github/dependabot.yml`: npm ×3, cargo ×4, github-actions |

**The P0 *story* is not met either (#129).** Beyond the #4 checklist, the
substantive P0 exit (HACKATHON §4) is open:

| # | P0 exit | State | Blocker |
|---|---|---|---|
| 8 | An investor thaws **through** `credit-gate` (Token ACL) | ⚠️ | ✅ on a validator with the real programs (`ops/gate/test-localnet.sh`); on devnet NPCS's freeze authority is still the `2xhj…` wallet and the gate is the pre-#26 build (#45) |
| 9 | SAS attestation read by the gate | ✅ | credential + schemas live on devnet (`ops/gate/deployment-devnet.json`); CG-P6 read tested against the real SAS program (#122) |
| 10 | R5 settles **live NPCS** | ❌ | R5 ran against a manifest mint (`payment-rails/PHASES.md`); waits on row 8 |
| 11 | Auction session + queue-claim sale | ⚠️ | both built and tested end to end in LiteSVM against the real rails, gate and SAS: a claim sells in an auction session (#136, #138). Not deployed |

## Repo structure

```
nundina/
├── token/                    ← THE ASSET: NPCS pilot token (docs, scripts, params, runbook)
│   ├── README.md             ← overview + live mint facts + verify how-to
│   ├── ARCHITECTURE.md       ← mint anatomy, creation sequence, movement topology (mermaid)
│   ├── TOKENOMICS.md         ← the three invariants + the exact math (mermaid)
│   ├── runbook.md            ← ops procedures + verified Token-2022 mechanics
│   ├── params.toml           ← every parameter, single source of truth
│   ├── scripts/init-mint.ts  ← creates the mint (exact extension manifest)
│   ├── scripts/verify.ts     ← asserts manifest + provenance on a live mint
│   └── keys/                 ← gitignored keypairs + deployment records
├── programs/                 ← Anchor workspace (solana programs + shared math)
│   ├── credit-gate/          ← Gate Program for Token ACL (sRFC37) — audit-scope-critical
│   ├── mark-engine/          ← NAV feeds, mark states, prints, MARK_PROTECTIVE (#137)
│   ├── auction-engine/       ← sealed-bid uniform-price sessions over payment-rails (#136)
│   ├── liquidity-facility/   ← advances at MARK_PROTECTIVE + liquidation ladder (#139)
│   ├── queue-claim/          ← redemption-queue claims: sell or pledge (#138)
│   ├── tranche-engine/       ← senior/junior tranching, three-book accounting (#140)
│   ├── npcs-treasury/        ← I1 mint-on-deposit treasury: the only NPCS mint path (#148)
│   └── credit-math/          ← shared fixed-point arithmetic — every money path routes here
├── payment-rails/            ← USDC money layer: escrow, DvP cash leg, payouts (own workspace; live on devnet)
│   ├── programs/payment-rails/ ← the Anchor program — audit-scope-critical
│   ├── tests-svm/            ← LiteSVM integration tests (own workspace + lockfile)
│   ├── fuzz/                 ← fuzz targets for the cash math (cargo-fuzz; stable smoke in CI)
│   ├── crank/                ← settlement/refund crank, reconciliation ledger, evidence export (TS)
│   └── DEPLOYMENTS.md        ← payment-rails deployment registry
├── app/                      ← Next.js 16 UI (pilot dashboards, session views)
├── ops/gate/                 ← credit-gate onboarding: SAS credential/schemas/attestations, Token ACL wiring (#122)
├── ops/squads/               ← Squads v4 2-of-2 authority migration + record (#126)
├── services/                 ← off-chain services: deferral record only (none built, #58)
├── preresearch/              ← the research corpus that justifies every design decision
│   ├── architecture/         ← 00 index · 05 problem register · 06 solution · 07 legal · 08 SRS · 09 asset spec
│   └── research/             ← 12 · dated market evidence (2026 reality check)
├── DEPLOYMENTS.md            ← official registry: every program + mint, independently verifiable
└── Anchor.toml               ← workspace config (anchor 1.1.2 · solana 2.3.0)
```

## Verify it yourself

```bash
# the shared math crate — unit + property tests
cargo test --workspace

# the live NPCS token on devnet — full manifest + provenance audit
cd token && npm install && CLUSTER=devnet npm run verify

# the deployed credit-gate program
solana program show 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX -u devnet

# the live payment-rails program: keyless, read-only; checks source id,
# genesis, build SHA-256, upgrade authority and slot against the registry
bash payment-rails/crank/scripts/devnet/verify.sh
```

## Reading order for a newcomer

1. [`token/TOKENOMICS.md`](token/TOKENOMICS.md) — what NPCS is and the three invariants (15 min)
2. [`token/ARCHITECTURE.md`](token/ARCHITECTURE.md) — how the token is built on Solana (15 min)
3. [`preresearch/research/12-Validation-2026-Reality-Check.md`](preresearch/research/12-Validation-2026-Reality-Check.md) — why now, with dated evidence
4. [`preresearch/architecture/05-Problem-Register.md`](preresearch/architecture/05-Problem-Register.md) — what kills this, ranked
5. [`preresearch/architecture/06-Solution-Architecture-v2.md`](preresearch/architecture/06-Solution-Architecture-v2.md) — the six-program design
6. [`preresearch/architecture/08-SRS-Pilot-Rails.md`](preresearch/architecture/08-SRS-Pilot-Rails.md) — the build spec (12 acceptance scenarios)

All diagrams are Mermaid — they render natively on GitHub.

## Position

- **Be the mechanism, not the venue.** The venue layer is commoditizing; the pricing/queue/risk-transfer layer is empty.
- **Never put a passive LP in front of a stale NAV.** Batch auction or nothing.
- **Compliance at account activation, not at transfer.** Token ACL + SAS, not transfer hooks.
- **The queue is the asset.** A pro-rated redemption request is a dated, priceable, financeable claim nobody has tokenized on any chain.

## License

MIT — see [LICENSE](LICENSE). Security-sensitive findings: see [SECURITY.md](.github/SECURITY.md) for responsible disclosure.
