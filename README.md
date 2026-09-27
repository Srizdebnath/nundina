# NUNDINA

> **The pricing, queue & risk-transfer engine for gated, NAV-priced private credit — on Solana.**
> Not a venue. Not an AMM. The mechanism that tells a quarterly-marked, gated credit position what it's worth — and turns a denied redemption request into a priceable, financeable claim.

[![Status](https://img.shields.io/badge/status-building_P1-blue)](preresearch/)
[![Phase](https://img.shields.io/badge/P0-scaffold_complete-success)](#repo-structure)
[![Token](https://img.shields.io/badge/NPCS-live_on_devnet-8A2BE2)](token/)
[![Docs](https://img.shields.io/badge/docs-10_files_~200KB-success)](preresearch/)
[![License: MIT](https://img.shields.io/badge/license-MIT-lightgrey)](LICENSE)

---

## The one-paragraph version

Tokenized private credit funds (ACRED, STAC, ONyc, SCOPE…) are locked, gated at ~5% per quarter, and NAV-priced only quarterly by the manager itself. In 2026 the gate crisis became the market's defining event — Apollo capped redemptions at 17% of NAV requested vs. a 5% cap; $5B+ sits trapped. NUNDINA builds the six-program mechanism nobody else has built: a **sealed-bid uniform-price batch auction** that manufactures an arms-length price where a passive AMM would bleed to adverse selection, a **mark engine** that makes stale appraisals usable, a **queue-claim** that tokenizes denied redemption requests, a **liquidity facility** that can actually liquidate permissioned collateral, a **credit-loss tranching waterfall**, and a **compliance gate** (Token ACL / sRFC37) enforcing per-lot Rule 144 seasoning, holder caps, and jurisdiction policy at account activation.

## Build status — real, on devnet

Nothing here is a mock. The pilot runs on a real, self-issued, USDC-backed asset.

| Component | Status | Where to verify |
|---|---|---|
| `credit-math` — shared fixed-point crate (NAV, clearing, waterfall, PV) | ✅ built · 41 tests + 7 property suites green | `cargo test --workspace` |
| `credit-gate` — sRFC37 Gate Program skeleton (CG-P2 config anchor) | ✅ **deployed devnet** | `DEPLOYMENTS.md` |
| **NPCS** — the pilot asset (Token-2022, frozen-by-default, TLV metadata) | ✅ **live devnet** · manifest verified | [`token/`](token/) · `DEPLOYMENTS.md` |
| Next.js 16 app shell | ✅ builds | `app/` |
| CI (fmt · clippy `-D warnings` · tests · app build) | ✅ green | `.github/workflows/ci.yml` |
| CG-P1..P6 gate logic + Token ACL wiring | 🔨 P1, in progress | SRS 08 §4.1 |
| mark-engine · auction-engine · queue-claim · facility · tranching | scheduled P2–P5 | SRS 08 §12 |

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
│   └── credit-math/          ← shared fixed-point arithmetic — every money path routes here
├── payment-rails/            ← USDC money layer: escrow, DvP cash leg, payouts (own workspace; live on devnet)
│   ├── programs/payment-rails/ ← the Anchor program — audit-scope-critical
│   ├── tests-svm/            ← LiteSVM integration tests (own workspace + lockfile)
│   ├── crank/                ← settlement/refund crank, reconciliation ledger, evidence export (TS)
│   └── DEPLOYMENTS.md        ← payment-rails deployment registry
├── app/                      ← Next.js 16 UI (pilot dashboards, session views)
├── preresearch/              ← the research corpus that justifies every design decision
│   ├── architecture/         ← 00 index · 05 problem register · 06 solution · 07 legal · 08 SRS · 09 asset spec
│   └── research/12           ← dated market evidence (2026 reality check)
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
solana program show 7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c -u devnet
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
