# NUNDINA × Crypto World's Fair (Colosseum) — Track Map

> Compiled **25 Sep 2026** from `preresearch/` + the live Colosseum page, the **official rules PDF**, and the Superteam Earn API (all 33 sidetracks read one by one).
> **Hard deadline: 12 Oct 2026, 11:59 PM PT = 13 Oct 2026, 12:29 PM IST.** That is **~17 days** from today.

---

## 1 · What we're building (short version)

**NUNDINA** is the pricing, queue and risk-transfer engine for gated, NAV-priced private credit on Solana. It is **not a venue or an AMM**.

- **Problem:** tokenized private-credit funds (ACRED, STAC, ONyc, SCOPE…) are locked, gated at about 5% of NAV per quarter, and the manager marks them only once a quarter. In 2026 the gates became the story: Apollo got redemption requests of 17% of NAV against a 5% cap, and $5B+ is trapped.
- **Gap:** Securitize+Jump+Jupiter, Orca permissioned pools, Exponent tranching and Uniswap permissioned pools all shipped *venues* in May–July 2026. **Nobody built the mechanism** to price a stale-NAV gated asset, tokenize the redemption queue, or liquidate permissioned collateral.
- **Six Anchor programs + one shared crate:**

| Program | What it does |
|---|---|
| `credit-gate` | Gate Program for Token ACL (sRFC37) that reads SAS attestations. Enforces eligibility, Rule 144 per-lot seasoning, a 12(g) holder cap and jurisdiction checks **when an account is activated**, not on every transfer |
| `auction-engine` | Sealed-bid, periodic, uniform-price batch auction (commit → reveal → clear → pro-rata → DvP settle → print). It has no passive LP, so there is nothing to pick off |
| `mark-engine` | Turns a stale NAV into a usable mark: discount curve, haircuts, and Fresh/Warned/Stale/Suspended states. **It never liquidates on a stale mark** |
| `queue-claim` | **The novel primitive.** A tokenized, pro-rated redemption request that can be sold in an auction or pledged for a loan |
| `liquidity-facility` | NAV-collateralized advance with a liquidation ladder: issuer ROFR → backstop bidder → forced-sale auction → workout |
| `tranche-engine` | Senior/junior credit-loss waterfall with three-book accounting |
| `credit-math` | Shared crate: u128 math, clearing, pro-rata and waterfall. Every rounding goes against the initiator |

> **Plus a seventh program built during the hackathon** — `payment-rails`, the USDC money layer (escrow, bonds, refunds, atomic DvP settlement, allowlisted payouts). It is not in SRS `08`; see **§4.1** for what it covers, what it deliberately does not, and the three places `08` needs amending because of it.

- **v1 go-to-market:** an issuer-sponsored liquidity window, meaning the issuer runs the auction on its own securities as a buyback tool.
- **Pilot asset:** NPCS (NUNDINA Pilot Credit Sleeve), a real Token-2022 token backed 1:1 by USDC in a Kamino vault, with NAV genuinely computed and published through our own SAS attester.

Full detail: `preresearch/architecture/06-Solution-Architecture-v2.md`, `08-SRS-Pilot-Rails.md`, `preresearch/research/12-Validation-2026-Reality-Check.md`.

---

## 2 · India eligibility ✅ confirmed

- **Main hackathon:** the official rules (§3b) exclude only Afghanistan, Belarus, Cuba, Iran, North Korea, Russia, Somalia, Syria, occupied regions of Ukraine, Venezuela and Yemen. **India is eligible.** You must be 18+, and each member can be on only **one team**, which can submit only **one project**.
- **Superteam India track** (listing `region: "India"`, status OPEN), with these conditions:
  1. Built on **Solana** (the pool is Solana-only even though the hackathon is multichain)
  2. **Team based in India**
  3. Registered on Colosseum with **"India" selected as your country** ← every member should check their profile
  4. Submitted to **both** the Colosseum portal **and** the Superteam Earn listing
  5. Eligible under the global rules

---

## 3 · Tracks we fit (ranked)

### A. Main Colosseum prizes (one submission covers all of these)

| Track | Prize | Fit | Why |
|---|---|---|---|
| **Solana Ecosystem track** | $100K (10 × $10K) | 🟢 **Primary** | Solana-native: Token ACL, SAS, Token-2022, Kamino integration |
| **Grand Champion** | $30K | 🟢 Reach | Novel primitive (queue-claim) aimed at a live $5B+ problem |
| **Top 20 standout teams** | $15K each ($300K) | 🟢 Realistic target | Scores well on the rules' criteria (see §5) |
| **Public Goods Award** | $5K | 🟡 Stretch | Only if we pitch the open-source `credit-gate` Gate Program and `credit-math` as reusable compliance infra for any RWA issuer on Solana (the repo is MIT) |
| **University Award** | $5K | ❓ Only if the team is students | Tell me and I'll add it |
| **Accelerator** | $250K pre-seed | 🟢 The real prize | All winners are interviewed for Colosseum's accelerator |
| Ethereum / Base / Arbitrum / Robinhood / Tempo / Hyperliquid / Zcash | $25K–$100K each | 🔴 Skip | We're Solana-native. Adding another chain would dilute a 17-day build |

### B. Superteam Earn sidetracks we can enter

Submit to each of these on Superteam Earn **in addition to** the Colosseum portal. All close **13 Oct 2026, 06:59 UTC**.

| Sidetrack | Prize | Fit | What it needs from us |
|---|---|---|---|
| **Superteam India** | $5K USDG (🥇 2.5K / 🥈 1.5K / 🥉 1K) + Superteam India Member role | 🟢 **Must enter** | Colosseum country = India. Judged on ecosystem impact, PMF (clear problem, defined user, early traction) and growth potential. Stack it on top of anything we win globally |
| **CertiK Security Audit Credits** | $100K in audit credits (10 × $10K) | 🟢 **Strong fit** | They want DeFi/RWA projects handling funds and planning mainnet + a raise. Our SRS already requires a full external audit before mainnet (§11), so this could pay for it. Submission: Colosseum link, repo, **programs to audit + approx LOC**, mainnet date, **6–12 month roadmap**, team (full-time?), **fundraising plan**, contact for a scoping call |
| **Adevar Labs Pre-Audit** | $20K (5 × $4K pre-audits) | 🟢 **Strong fit** | Solana/Rust submissions only. Judged on codebase complexity, architectural clarity, repo completeness and launch readiness. Requires a short form, a **tweet**, and following @AdevarLabs |
| **RPC Fast Infrastructure** | ~$10.5K in RPC credits (~$500 per team × 21) | 🟡 Easy add-on | Claim the free Focus plan (mainnet only, Frankfurt), use it, and **post 2–3 times a month about RPC Fast for 2 months**. Low value but low effort |
| **Solami — live Solana data** | $3K USDG (1.2K / 1K / 500 / 300) | 🟡 Conditional | Solami has to be the actual data path for the indexer (Yellowstone gRPC / webhooks instead of Helius). The app must **run live on mainnet**, with a public repo and a 2–3 min live-mainnet demo. Only worth it if we deploy a mainnet slice |

### C. Sidetracks we should skip

| Sidetrack | Why skip |
|---|---|
| Meteora DBC ($20K) | It's a bonding-curve launchpad, which contradicts our core rule: never put a passive LP in front of a stale NAV |
| Panta API ($5K) | Prediction markets, which aren't our product |
| peaq ($4K) | Germany-only, and about the machine economy |
| Other regional Superteam tracks (Australia, Vietnam, Argentina, Türkiye, Ukraine, Thailand, Georgia, UK, Canada, Japan, UAE, Kazakhstan, Singapore, Ireland, Germany, Poland, Malaysia ×2, Nigeria, NL, Nepal, Brasil, Balkan, SolanaCZE, La Familia) | Country-locked. SolanaCZE and Vietnam are tagged "Global" in the API, but their text requires a Czech/Slovak member or Vietnam's local program |

### D. Multichain: Tempo stretch only

Decision (25 Sep 2026): **the core stays Solana-only.** The one extra chain we'll attempt is **Tempo, as a payout-only stretch goal.**

- **Why not a multichain core:** `credit-gate` depends on Token ACL (sRFC37) and SAS, which exist only on Solana. Moving the restricted fund token to another chain takes it outside the compliance gate, which is exactly the posture `preresearch/architecture/07` warns against. A second EVM stack would also cost days we don't have, and judges mark down bolted-on integrations.
- **Tempo** ($100K, 10 × $10K): Tempo is a stablecoin payments chain. Queue-claim sale proceeds, facility advances and tranche coupons are **plain USDC, never the security**, so a holder can choose a Tempo address as the payout destination. The regulated asset never leaves Solana, and only the cash crosses. Estimated effort is 2–3 days. **Start only after P0 works end to end on devnet (target ~day 12).**
- **Hyperliquid:** skip. The natural build is a perp on a gated-credit discount index, but a derivative on private securities is a legal minefield (security-based swap), and launching a HIP-3 market needs a large HYPE stake. Publishing the index as data alone is too weak to compete.
- **Ethereum / Robinhood Chain:** most tokenized private credit lives on EVM, so this goes on the **roadmap slide** ("v2: EVM gated funds"), not in the build.
- **Zcash / Base / Arbitrum:** no meaningful fit.
- ⚠️ **Open question for Colosseum (Discord / Copilot):** can one project win **more than one ecosystem track** (Solana + Tempo)? The rules allow only one project per team (§7) and don't answer this. If the answer is no, drop Tempo.

### Realistic prize stack

Solana track ($10K) + Superteam India (up to $2.5K) + CertiK ($10K credits) + Adevar ($4K pre-audit) + RPC Fast credits, with Top-20 ($15K) and an accelerator interview as the upside.

---

## 4 · Hackathon scope: cut the 24-week SRS down to 17 days

The SRS (`08`) is a ~24-week plan that includes a 10-week audit. For the hackathon we ship a **devnet MVP that proves the novel parts**, and present the rest as the roadmap (which also feeds the CertiK and Adevar applications).

| Priority | Build | Proves (SRS AC) | Status @ `9cb25b0` |
|---|---|---|---|
| **P0 — must** | NPCS Token-2022 mint (DefaultAccountState=frozen) | — | ✅ **done** — live devnet `Dy1VW…dx2i`, all four `08` §3.1 extensions |
| **P0 — must** | Token ACL (`MintConfig` delegates thaw to the gate) | AC-1 | ❌ **not started** — freeze authority is still a plain wallet |
| **P0 — must** | `credit-gate` + SAS attestation. An eligible wallet thaws; an ineligible one gets a typed rejection | AC-1 | ⚠️ **~15%** — only CG-P2's config PDA (`initialize_config`/`update_config`). No `validate()`, no SAS read, no lot ledger, no governor, no block-list. See #5 |
| **P0 — must** | `auction-engine` + `credit-math`: commit → reveal → uniform-price clear → pro-rata → settle → print, with 3+ bidders | AC-3 / AC-4 | ❌ **not started** — no state machine, no clearing, no allocation, no print. The cash leg exists as `payment-rails` (see below). See #7 |
| **P0 — must** | `queue-claim`: mint a claim from a redemption request and **sell it in an auction session** | AC-8 (sell path): **the headline demo** | ❌ **not started** — blocked on the two rows above. See #8 |
| **P1 — should** | `mark-engine` (minimal): publish NAV, Fresh→Stale states, print → discount curve | AC-2 (simplified) | ❌ not started. See #6 |
| **P1 — should** | `liquidity-facility`: pledge a claim → USDC advance + "no liquidation on a stale mark" | AC-8 pledge, part of AC-9 | ⚠️ **cash leg only** — facility vault/advance/repay and the QC-P4 sweep order are done inside `payment-rails`; underwriting, `MARK_PROTECTIVE` pricing and the ladder are not. See #8 |
| **P2 — slides** | `tranche-engine` waterfall, full liquidation ladder, holder governor, lot seasoning | Roadmap slides | ⚠️ TE-P4's cash waterfall only. Correctly deprioritised — leave as slides |
| **UI** | Next.js: one session page (commit/reveal/clearing animation), a portfolio (claims) page, a gate-rejection view | 3-minute demo video | ❌ **not started** — `app/` is a 2-file scaffold. Do not start until the gate and the auction land. See #10 |

**Guardrails from our own docs:** stay on devnet, keep NPCS inside team wallets (the offering gate in `08` §10.3), label everything `PILOT-DEMO-ASSET`, and never fake a mainnet markdown.

### 4.1 · The `payment-rails` pivot — a 7th program not in the SRS

While building, the team scoped a program that appears in **neither** this table nor `08` §4:

| `payment-rails` | What it does |
|---|---|
| What it is | The **USDC money layer** only. Escrow vaults, forfeitable bonds, permissionless refunds, atomic DvP settlement, fee/reserve split, issuer ROFR, and a payout router that pays **only attested, allowlisted wallets** (hash-only, no PII on-chain). |
| Why | The security never leaves Solana; **only cash moves**. Splitting the cash layer out means the auction, facility and tranching engines can all call into one audited money path instead of each re-implementing escrow. |
| State | ✅ **~95% and live on devnet** (`GdWm…Pocn`). R0–R5 merged in #20–#25. 5.6k LoC Rust + 3.4k LoC LiteSVM tests + 2.1k LoC TS crank/ledger/evidence-export. `cargo fmt`/`clippy -D warnings` clean, 18/18 unit tests green. |
| What it does **not** do | No auction state machine, no clearing, no allocation, no print, no gate, no SAS. The operator **hand-picks each fill** and is trusted to have applied the clearing correctly — that trust assumption is stated in `payment-rails/INTERFACE.md` and is the seam the real `auction-engine` must fill. |
| Real AC progress | **AC-11's liveness property is genuinely proven** — `payment-rails/crank/scripts/drill.ts` kills the crank mid-session on a real validator, restarts, proves no double-settlement (`fills_settled === 2`), and completes every refund from a **third-party participant's wallet**. Plus the DvP cash leg of AC-3. |

**Consequences for `08`, which has not been amended:**

1. **`08` §4.2 no longer owns AE-P3/P4/P7/P8/P10.** They now live in a different program with its own operator trust model. `08` §15's traceability matrix is stale.
2. **`08` §11's frozen audit scope is wrong.** It names `auction-engine` (which does not exist) and omits `payment-rails` (which moves every micro-USDC, by its own admission at `payment-rails/programs/payment-rails/src/lib.rs:4-6`). A CertiK or Adevar submission built off §11 would omit the actual money path. **Fix before requesting quotes** — see #11 for a proposed re-frozen scope.
3. **Auditing scope grew, it did not shrink.** The LOC in scope is up by 5.6k lines of value-moving code, so the 6–10 week estimate in `08` §11 must be re-quoted, not assumed.

### 4.2 · Open blockers, in the order they should be fixed

| # | Blocker | Why it is first |
|---|---|---|
| [#26](https://github.com/Srizdebnath/nundina/issues/26) | 🔴 **`credit-gate::initialize_config` is front-runnable** — the first caller for any mint becomes its permanent gate authority, and can then rewrite the SAS attester set, holder cap and block-list | Silently disables CG-1/2/4/5/6. Must be fixed **before** the gate is built on top of it, and before an auditor sees it |
| [#27](https://github.com/Srizdebnath/nundina/issues/27) | 🔴 **`payment-rails` has no CI at all** — 5.6k LoC Rust + 2.1k LoC TS, live on devnet, zero automated build/test | Nothing catches an `error.rs` renumber, a `state.rs` field reorder or a breaking `@solana/kit` bump. Everything else below is built on this |
| [#29](https://github.com/Srizdebnath/nundina/issues/29) | 🔴 **Program keypair is gitignored with no backup**, and the upgrade authority is a single non-Squads key | Losing the file permanently bricks the program ID. Squads 2-of-2 is required by `08` §8 before any mainnet deploy |
| [#28](https://github.com/Srizdebnath/nundina/issues/28) | 🟠 **`deploy.sh` cluster-label footgun** — `RPC_URL` without `CLUSTER` can deploy to mainnet and record it as `devnet` in the public registry | A false statement in a published artifact |
| — | 🟠 **`PermanentDelegate` vs refund liveness is unresolved** (`payment-rails/PHASES.md` Q4) | NPCS's permanent delegate can pull asset from an ask vault; `refund_ask` then fails `ConservationViolated` forever, stranding the seller's bond — contradicting AE-P10. Bounded on a team-only pilot; solvency-relevant the moment real holders exist |

**Honest status: 0 of 12 acceptance criteria are met.** The P0 list above is largely unstarted, and 10 of the 12 ACs depend on programs that do not yet exist. The rails are real, tested work on the money path — but they are not a substitute for the gate and the auction, which are what the demo actually needs to show.

---

## 5 · How judges score it (rules §8), and our answer

| Criterion | Our answer |
|---|---|
| **Functionality / code quality** | Working devnet programs, property-tested `credit-math`, clean Anchor |
| **Potential impact / TAM** | $5B+ trapped behind gates, ~$8B tokenized private credit, 1–20% secondaries discounts (research `12`) |
| **Novelty** | Queue-claim: a pro-rated redemption request as a tokenized, financeable claim. Nobody has shipped this on any chain |
| **UX** | Denied redemption → exit or loan in one flow. Institutional dark UI |
| **Open-source / composability** | MIT license. Composes with Token ACL, SAS, Kamino, Orca, RedStone |
| **Business plan** | Issuer sponsorship license + facility spread + discount-curve data product. No trading fees (`06` §13) |

---

## 6 · Submission checklist

- [ ] **Every** team member registers on colosseum.com, **country = India**, before 12 Oct 11:59 PM PT
- [ ] Team leader uploads the project to the Colosseum portal. All content must be in **English**
- [ ] Public GitHub repo (MIT) + README with run instructions
- [ ] Pitch video + demo video (~3 min each is the norm across sidetracks)
- [ ] Superteam Earn: **Superteam India** listing (name, description, GitHub, website…)
- [ ] Superteam Earn: **CertiK** (roadmap, LOC, fundraising, team, contact)
- [ ] Superteam Earn: **Adevar Labs** + tweet + follow @AdevarLabs
- [ ] Optional: **RPC Fast** (claim plan, follow @rpcfast, commit to posting), **Solami** (only if live on mainnet)
- [ ] Sidetrack listings are `HUMAN_ONLY`, so a human submits them, not an agent

Winners are announced by **5 Dec 2026**. Superteam India's commitment date is 28 Oct 2026.

---

## Sources

- Colosseum: https://colosseum.com/worldsfair and the [official rules PDF](https://colosseum.com/legal/Crypto%20World's%20Fair%20Hackathon%20Rules.pdf) (prizes §14, eligibility §3, judging §8, timing §5)
- Superteam Earn: https://superteam.fun/earn/hackathon/crypto-worlds-fair, with the listing details pulled from `superteam.fun/api/listings/details/<slug>`
- Project context: `README.md`, `preresearch/architecture/00, 05–08`, `preresearch/research/12`
