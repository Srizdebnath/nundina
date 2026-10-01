# Architecture — Private Credit Secondary Markets on Solana

> **⚠️ READ THIS FIRST.** Docs `01`–`04` were written against the market as of early 2026 and are **superseded**. Between 5 May and 24 June 2026 the ecosystem shipped three of the four things those docs assume nobody had built:
> - **Orca permissioned pools** (27 May 2026) — a permissioned AMM for regulated tokenized securities on Solana
> - **Securitize + Jump + Jupiter** (5 May 2026) — an ATS-backed regulated secondary market on Solana
> - **Exponent Finance V2** (24 June 2026) — the first senior/junior risk-tranching market on Solana
>
> Start with `../research/12-Validation-2026-Reality-Check.md`, then `05`, then `06`.

**Current thesis:** not a venue, not an AMM, not a compliance registry — **the pricing, queue and risk-transfer engine for gated, NAV-priced, thin-float credit**, delivered v1 as an **issuer-sponsored liquidity window**.

## How to read this folder

| File | What's inside | Status |
|---|---|---|
| **`../research/12-Validation-2026-Reality-Check.md`** | The adversarial validation: what's now solved, what's still open, whether Solana is right, and the repositioned thesis — with dated evidence | ✅ **Start here** |
| **`05-Problem-Register.md`** | The complete failure inventory: 141 problems across 13 domains, each with failure mechanism, severity, likelihood, and a pointer to its answer. Ends with the ranked ten that decide the outcome | ✅ **Current** |
| **`06-Solution-Architecture-v2.md`** | The architecture that answers every row of `05`, with diagrams per problem cluster — auction engine, mark engine, queue-claim, liquidity facility, tranche engine, credit gate, lot ledger, legal topology, circuit breakers, GTM — plus the `payment-rails` cash layer that holds all client cash (§1 amendment, #47). Ends with a full traceability matrix | ✅ **Current** |
| **`07-Legal-Topology-and-Compliance.md`** | The linked legal + engineering compliance layer: full C-domain legal risk register (L1–L27; note that doc 05 reuses `L1`–`L6` for *governance* problems, so cite `07 L4` or `05 L4` when it matters) with per-rule counsel questions, legal topology per phase (v1 issuer window → v2 partner ATS → v3 SPV/offshore), per-program compliance requirements (CG/AE/QC/LF/TE/ME), 20 counsel gates ready to hand to a securities firm, and an ongoing compliance calendar | ✅ **Current** |
| **`08-SRS-Pilot-Rails.md`** | Build-level SRS for the pilot: a real USDC-backed self-issued pilot asset (NPCS) proving all seven programs (six engines + the `payment-rails` cash layer) end-to-end — real SAS attestations, Token ACL gating, sealed-bid auctions, queue-claims, facility ladder, tranching, full external audit before mainnet, Next.js 16 UI, 12 acceptance scenarios, ~4–5 month phase plan for a 1–2 person team | ✅ **Current** |
| **`09-NPCS-Asset-Spec.md`** | The pilot asset at decision level: NPCS token structure (extension manifest), the three invariants (backing / NAV / eligibility) with exact integer math, gate wiring order, repo layout, key ceremony, deployment plan | ✅ **Current** |
| `01-High-Level-Architecture.md` | Original layered view. Assumes six programs including `credit-amm`, `credit-book`, `credential-registry`, `nav-oracle` | ⛔ Superseded by `06` |
| `02-Protocol-Components.md` | Original program-by-program flows | ⛔ Superseded by `06` |
| `03-Feasibility-and-Compliance.md` | Original why-Solana case. **Two load-bearing claims are wrong:** transfer hooks *break* rather than enable composability (use Token ACL / sRFC37), and "permissioned = compliant by construction" ignores Exchange Act Rule 3b-16 venue registration | ⛔ Superseded by `06` §3 and §9 |
| `04-Competitive-Differentiation.md` | Original competitive map. The "nobody on Solana does this" claims expired in May–June 2026 | ⛔ Superseded by `../research/12` |

## Design pillars (v2)

1. **Be the mechanism, not the venue.** Orca and Securitize built venues. Nobody built the thing that prices a gated, quarterly-marked credit position.
2. **Never put a passive LP in front of a stale NAV.** Price discovery is a sealed-bid periodic uniform-price auction — no pool to pick off, so the adverse selection that has blocked this whole category never arises.
3. **Compliance at account activation, not at transfer.** Token ACL (sRFC37) + Solana Attestation Service, not transfer hooks.
4. **v1 is an issuer-sponsored liquidity window.** One structural decision that resolves the five worst problems in the register at once: issuer consent, issuer incentive, venue registration, discount signalling, and one-sided flow.
5. **The queue is the asset.** The binding constraint in 2026 is the ~5% quarterly gate, not the lock-up. A pro-rated redemption request is a dated, priceable, financeable claim that nobody has tokenized on any chain.
6. **Integrate, don't rebuild.** NAVLink/RedStone for NAV, SAS for credentials, Kamino/Loopscale for lending, Orca for continuous trading of tranche tokens.

## Mermaid rendering

All diagrams are GitHub/Mermaid-flavored. They render on GitHub, mermaid.live, Obsidian, and VS Code (Mermaid extension).
