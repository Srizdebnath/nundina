# NUNDINA

> **The pricing, queue & risk-transfer engine for gated, NAV-priced private credit — on Solana.**
> Not a venue. Not an AMM. The mechanism that tells a quarterly-marked, gated credit position what it's worth — and turns a denied redemption request into a priceable, financeable claim.

[![Status](https://img.shields.io/badge/status-preresearch_complete-blue)](preresearch/)
[![Next](https://img.shields.io/badge/next-pilot_build_(SRS_08)-orange)](preresearch/architecture/08-SRS-Pilot-Rails.md)
[![Docs](https://img.shields.io/badge/docs-9_files_~150KB-success)](preresearch/)
[![License: MIT](https://img.shields.io/badge/license-MIT-lightgrey)](LICENSE)

---

## The one-paragraph version

Tokenized private credit funds (ACRED, STAC, ONyc, SCOPE…) are locked, gated at ~5% per quarter, and NAV-priced only quarterly by the manager itself. In 2026 the gate crisis became the market's defining event — Apollo capped redemptions at 17% of NAV requested vs. a 5% cap; $5B+ sits trapped. NUNDINA builds the six-program mechanism nobody else has built: a **sealed-bid uniform-price batch auction** that manufactures an arms-length price where a passive AMM would bleed to adverse selection, a **mark engine** that makes stale appraisals usable, a **queue-claim** that tokenizes denied redemption requests, a **liquidity facility** that can actually liquidate permissioned collateral, a **credit-loss tranching waterfall**, and a **compliance gate** (Token ACL / sRFC37) enforcing per-lot Rule 144 seasoning, holder caps, and jurisdiction policy at account activation. v1 ships as an **issuer-sponsored liquidity window** — the issuer runs the auction on its own securities as a NAV-accretive buyback tool.

## Why this exists now (Sept 2026)

- **Venues shipped; mechanisms didn't.** Securitize+Jump+Jupiter, Orca permissioned pools, and Exponent tranching all launched May–June 2026. None prices a stale-NAV gated asset, tokenizes the redemption queue, or liquidates permissioned collateral. That layer is empty.
- **The gate crisis is live.** 5% quarterly caps enforced industry-wide, ~$5B+ trapped, defaults ~10% — the demand side is bleeding exactly where this product operates.
- **The rails matured.** Token ACL (sRFC37), Solana Attestation Service, RedStone NAV feeds, Anchorage/Fireblocks SPL custody — everything the architecture integrates now exists.
- **A regulatory template landed.** The SEC's TSV order (Sept 17, 2026) legitimizes permissioned on-chain venues (equities scope — never load-bearing here, but the direction is set).

Full evidence: [`preresearch/research/12-Validation-2026-Reality-Check.md`](preresearch/research/12-Validation-2026-Reality-Check.md).

## Repo map

```
preresearch/
├── architecture/
│   ├── 00-README.md                        ← start here: doc index + design pillars
│   ├── 01–04  (superseded v1 docs)         ← kept for history; read 06 instead
│   ├── 05-Problem-Register.md              ← ~100 failure modes, 13 domains, ranked kill-list
│   ├── 06-Solution-Architecture-v2.md      ← the answer to every row of 05; six programs
│   ├── 07-Legal-Topology-and-Compliance.md ← L1–L27 legal register + 20 counsel gates
│   ├── 08-SRS-Pilot-Rails.md               ← build spec: 12 acceptance scenarios, 24-week plan
│   └── 09-NPCS-Asset-Spec.md               ← the pilot asset: token structure, invariants, deployment
└── research/
    └── 12-Validation-2026-Reality-Check.md ← dated market evidence, competitor map, TSV order
```

## Reading order for a newcomer

1. `preresearch/architecture/00-README.md` — thesis, pillars, index
2. `preresearch/research/12-Validation-2026-Reality-Check.md` — why now, with sources
3. `preresearch/architecture/05-Problem-Register.md` — what kills this, ranked
4. `preresearch/architecture/06-Solution-Architecture-v2.md` — the design that answers it
5. `preresearch/architecture/07-Legal-Topology-and-Compliance.md` — how it stays legal
6. `preresearch/architecture/08-SRS-Pilot-Rails.md` — what we're building next

All diagrams are Mermaid — they render natively on GitHub.

## Next milestones

| Milestone | Doc |
|---|---|
| Pilot build P0–P5: gate + asset + marks + auction + claims + facility + tranching | `08` §12, `09` |
| External audit (gate / auction / facility / math) | `08` §11 |
| Mainnet pilot: 12 acceptance scenarios, real USDC, recorded demo | `08` §1, §12 |
| Counsel gates Q1/Q3/Q7 (venue, exemption, repurchase) | `07` §Part IV |
| First issuer window (OnRe → Figure/Hastra → Securitize) | `06` §12 |

## Position

- **Be the mechanism, not the venue.** The venue layer is commoditizing; the pricing/queue/risk-transfer layer is empty.
- **Never put a passive LP in front of a stale NAV.** Batch auction or nothing.
- **Compliance at account activation, not at transfer.** Token ACL + SAS, not transfer hooks.
- **The queue is the asset.** A pro-rated redemption request is a dated, priceable, financeable claim nobody has tokenized on any chain.

## License

MIT — see [LICENSE](LICENSE). Security-sensitive findings: see [SECURITY.md](.github/SECURITY.md) for responsible disclosure.
