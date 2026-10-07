# 11 · Securities counsel engagement brief (Q1–Q20)

**Status (2026-10-07, #150): brief written, counsel not engaged.** Doc 07
lists 20 counsel gates (Q1–Q20). They expired by standing open: several
pilot decisions were taken on engineering defaults (7-day seasoning, no
per-trade fee, US-only jurisdiction policy) that counsel has to confirm or
change. This is the brief to send. Picking a firm, signing the engagement
letter and paying for it are the founders' actions.

## Engagement log

| Step | Owner | Date | State |
|---|---|---|---|
| Shortlist 2–3 US securities firms with digital-asset and private-fund practices | founders | — | ❌ |
| Send this brief + the documents below under NDA; ask for a fixed-fee quote on the **pilot set** | founders | — | ❌ |
| Engagement letter signed; conflicts cleared | founders | — | ❌ |
| Written memos received for the pilot set | counsel | — | ❌ |
| Code and doc changes from each memo, one PR per question (`Q<n>` in the title) | eng | — | ❌ |

## Who we are, in one paragraph

NUNDINA is software for the secondary liquidity of gated, NAV-priced
private-credit fund interests on Solana. It runs periodic sealed-bid
uniform-price auctions (no passive liquidity pool) and tokenizes pro-rated
redemption-queue requests as claims that can be sold or pledged. A liquidity
facility advances USDC against them, and a tranche engine splits senior from
junior risk. Eligibility is enforced when a token account is activated
(Token ACL + Solana Attestation Service), not at transfer. All client cash
sits in program-controlled escrow that no operator key can sign for. The v1
legal posture is an **issuer-sponsored liquidity window** (doc 07 Part II).
The pilot asset (NPCS) is self-issued, labelled `PILOT-DEMO-ASSET — NOT AN
OFFER`, held only by team-controlled attested wallets (SRS 08 §10.3), and
runs on devnet.

## Pilot set: what we need first, and what it unblocks

These seven gate the mainnet pilot (SRS 08 P8) or replace defaults already
in code. The rest are v2/v3 and can be quoted separately.

| Q | Question (full text: doc 07 §Counsel gates) | What is in code today that counsel must confirm or change |
|---|---|---|
| **Q3** | Do auction fills on seasoned, non-affiliate lots satisfy §4(a)(1½) / §4(a)(7) (sealed bids, accredited-only, per-lot seasoning, no general solicitation), and what disclosure must go with each fill? | `credit-gate` `settle_check` refuses unseasoned lots on every fill; each fill carries a `disclosure_hash` and an exemption-path code (`ExemptionPath`); the **offering gate** (SRS 08 §10.3) keeps NPCS in team wallets until Q3 clears |
| **Q4** | Rule 144 mechanics for non-reporting issuers: holding period, tacking through the auction, affiliate limits, the exact formula for the lot ledger | `MarketConfig.seasoning_seconds` = 7 days (a compressed pilot analog); lots carry `affiliate` and `exemption_path`, but no affiliate volume limit is enforced |
| **Q5** | §12(g) holder counting: on-chain holders, nominee holders, queue-claim holders | holder governor counts distinct `investor_id`s, `holder_cap` = 8 on devnet; rails escrow vaults are excluded from the count |
| **Q2** | Which revenue lines create transaction-based-compensation or finder risk? | per-trade fee removed (#147, `MAX_FEE_BPS` = 0); a ≤ 2% `reserve_bps` loss reserve remains: **confirm it is not compensation** |
| **Q10** | Is program-controlled, holder-revocable escrow custody for us or a partner BD? | all cash in PDA vaults; refunds permissionless and never pausable |
| **Q13** | BSA/MSB and OFAC: are we a money transmitter given non-custodial escrow and program settlement? Screening cadence? | block-list enforced at thaw and at every settle; no screening process exists yet |
| **Q14** | Which stablecoins are clean cash legs post-GENIUS; per-asset diligence | USDC only (`RailsConfig.usdc_mint`, depeg breaker) |

Then: **Q1** (exchange / broker status of the issuer window, also with
third-party orders), **Q6** (assignment of queue proceeds under the LPA), **Q7**
(repurchase-window discipline), **Q12** (Form D), **Q18** (selling the print
dataset), **Q19** (entity and team). v3 and later: Q8, Q9, Q11, Q15, Q16,
Q17. Standing: Q20.

## Documents to send

| Document | Why |
|---|---|
| `07-Legal-Topology-and-Compliance.md` | the legal risk register L1–L27, the topology per phase, every counsel question with the problems it closes |
| `06-Solution-Architecture-v2.md` §1, §3, §9 | the mechanism, the legal topology, the gate |
| `08-SRS-Pilot-Rails.md` §10 | how the pilot obeys its own rules; the offering gate |
| `09-NPCS-Asset-Spec.md`, `token/TOKENOMICS.md` | the pilot asset, what is and is not enforced on chain |
| `DEPLOYMENTS.md` | who holds which key (Squads 2-of-2, 48 h timelock) |

## Deliverable we ask for

A short written memo per question: the answer, the conditions it depends on,
and the exact parameter or behavior to encode (for example, the Rule 144
formula for Q4, the disclosure fields for Q3). Each memo becomes a PR that
changes code or docs and cites the memo.
