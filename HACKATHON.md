# NUNDINA × Crypto World's Fair (Colosseum) — Track Map

> Compiled **25 Sep 2026** from `preresearch/` + the live Colosseum page, the **official rules PDF**, and the Superteam Earn API (all 33 sidetracks read one by one).
> **Hard deadline: 12 Oct 2026, 11:59 PM PT = 13 Oct 2026, 12:29 PM IST.** That is **~17 days** from today.

---

## 1 · What we're building (short version)

**NUNDINA** is the pricing, queue and risk-transfer engine for gated, NAV-priced private credit on Solana. It is **not a venue or an AMM**.

- **Problem:** tokenized private-credit funds (ACRED, STAC, ONyc, SCOPE…) are locked, gated at about 5% of NAV per quarter, and the manager marks them only once a quarter. In 2026 the gates became the story: Apollo got redemption requests of 17% of NAV against a 5% cap, and $5B+ is trapped.
- **Gap:** Securitize+Jump+Jupiter, Orca permissioned pools, Exponent tranching and Uniswap permissioned pools all shipped *venues* in May–July 2026. **Nobody built the mechanism** to price a stale-NAV gated asset, tokenize the redemption queue, or liquidate permissioned collateral.
- **Seven Anchor programs + one shared crate** in the design (six engines plus the `payment-rails` cash layer, SRS 08 §4.7). **As built (2026-10): one program is live (`payment-rails`), one is partial (`credit-gate`: config, block-list and lot ledger, not yet on devnet), and four have no code at all** (#128). The Status column is the as-built truth; the description is the target:

| Program | Status (as built) | What it does (target) |
|---|---|---|
| `credit-gate` | ✅ on `main`, **not yet deployed**: CG-P1..P7: SAS attestation read, holder governor, block-list, lot ledger, `validate` and the Token ACL thaw/freeze hooks, tested against the real SAS and Token ACL programs (#122). Devnet still runs the pre-#26 skeleton until the `Bprm…` holder redeploys (#45, #146) | Gate Program for Token ACL (sRFC37) that reads SAS attestations. Enforces eligibility, Rule 144 per-lot seasoning, a 12(g) holder cap and jurisdiction checks **when an account is activated**, not on every transfer |
| `auction-engine` | 🗺 **roadmap**: no crate, IDL, test or crank | Sealed-bid, periodic, uniform-price batch auction (commit → reveal → clear → pro-rata → DvP settle → print). It has no passive LP, so there is nothing to pick off |
| `mark-engine` | 🗺 **roadmap**: no crate, IDL, test or crank | Turns a stale NAV into a usable mark: discount curve, haircuts, and Fresh/Warned/Stale/Suspended states. **It never liquidates on a stale mark** |
| `queue-claim` | 🗺 **roadmap**: no crate, IDL, test or crank | **The novel primitive.** A tokenized, pro-rated redemption request that can be sold in an auction or pledged for a loan |
| `liquidity-facility` | 🗺 **roadmap**: engine not built; only the cash leg (advance/repay/sweep) exists in `payment-rails`; no liquidation ladder (#124) | NAV-collateralized advance with a liquidation ladder: issuer ROFR → backstop bidder → forced-sale auction → workout |
| `tranche-engine` | 🗺 **roadmap**: engine not built; only the cash waterfall exists in `payment-rails` | Senior/junior credit-loss waterfall with three-book accounting |
| `payment-rails` | ✅ **live on devnet** | The USDC cash layer under the engines: per-escrow vaults and bonds, permissionless never-pausable refunds, atomic one-fill DvP cash leg, issuer ROFR, facility cash, coupon waterfall. It **holds all client cash**, so it is audit-scope-critical alongside `credit-gate` |
| `credit-math` | ✅ built, tested | Shared crate: u128 math, clearing, pro-rata and waterfall. Every rounding goes against the initiator |

- **v1 go-to-market:** an issuer-sponsored liquidity window, meaning the issuer runs the auction on its own securities as a buyback tool.
- **Pilot asset:** NPCS (NUNDINA Pilot Credit Sleeve), a real Token-2022 mint live on devnet. **As built (#120): supply is 0 and nothing backs it on-chain.** There is no deposit or mint-on-deposit path, no Kamino (or any) treasury vault, no NAV publisher and no SAS attester; invariant I1 (USDC 1:1) is a process rule, not code ([`token/TOKENOMICS.md` §0](token/TOKENOMICS.md)). *Target:* minted 1:1 against USDC in a Kamino supply vault (backup: a program-owned USDC PDA), with NAV computed from the sleeve and published through our own SAS attester.

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
| **Solana Ecosystem track** | $100K (10 × $10K) | 🟢 **Primary** | Solana-native: Token-2022 today; Token ACL, SAS and a Kamino sleeve are the target design, not yet integrated |
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
| **CertiK Security Audit Credits** | $100K in audit credits (10 × $10K) | 🟢 **Strong fit** | They want DeFi/RWA projects handling funds and planning mainnet + a raise. Our SRS already requires a full external audit before mainnet (§11), so this could pay for it. Submission: Colosseum link, repo, **programs to audit + approx LOC** (as built, 2026-10: `payment-rails` ~4.2k lines of Rust, the program holding client cash, plus its ~1.2k-line TS crank; `credit-gate` ~1.4k; `credit-math` ~1.3k), mainnet date, **6–12 month roadmap**, team (full-time?), **fundraising plan**, contact for a scoping call |
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

| Priority | Build | Proves (SRS AC) | State (2026-10, #128/#129) |
|---|---|---|---|
| **✅ built** | `payment-rails`: escrow + bonds, permissionless refunds, atomic DvP cash leg, ROFR, facility cash, coupon waterfall, crank + reconciliation + evidence export — live on devnet (R5) | AC-11 (refund liveness), cash legs of AC-3/AC-8 | ✅ met. R5 settled Circle devnet USDC against a **manifest mint, not live NPCS** (`payment-rails/PHASES.md`); the live-NPCS DvP waits on the gate thaw |
| **P0 — must** | NPCS Token-2022 mint (DefaultAccountState=frozen) + Token ACL + `credit-gate` + SAS attestation. An eligible wallet thaws; an ineligible one gets a typed rejection | AC-1: compliance at activation | ⚠️ **code + SAS done, not live on NPCS.** SAS credential + schemas live on devnet; `validate` + Token ACL hooks pass end to end on a validator with the real programs (`ops/gate/test-localnet.sh`). Waiting on the key holders: `credit-gate` redeploy (`Bprm…`, #45) and the NPCS freeze authority handed to Token ACL (`2xhj…`) |
| **P0 — must** | `auction-engine` + `credit-math`: commit → reveal → uniform-price clear → pro-rata → settle → print, with 3+ bidders | AC-3 / AC-4: price discovery at a discount to NAV | ❌ **unmet.** `credit-math` clearing + pro-rata ✅ (tested, no program consumer); `auction-engine` has no code |
| **P0 — must** | `queue-claim`: mint a claim from a redemption request and **sell it in an auction session** | AC-8 (sell path): **the headline demo** | ❌ **unmet, no code.** The headline demo is unshipped |
| **P1 — should** | `mark-engine` (minimal): publish NAV, Fresh→Stale states, and the auction print feeding the discount curve | AC-2 (simplified) | ❌ no code. NAV math exists in `credit-math`; nothing publishes it |
| **P1 — should** | `liquidity-facility`: pledge a claim → USDC advance, plus a demo of "no liquidation on a stale mark" | AC-8 pledge path, part of AC-9 | ⚠️ cash leg only: `payment-rails` advance/repay/sweep with the H1 50% ceiling. No pledge path, no mark, no liquidation, so "no liquidation on a stale mark" is untestable (#124) |
| **P2 — slides** | `tranche-engine` waterfall, full liquidation ladder, holder governor, lot seasoning | Roadmap slides | 🗺 roadmap (the cash waterfall alone is built in `payment-rails`) |
| **UI** | Next.js: one session page (commit/reveal/clearing animation), a portfolio (claims) page, and a gate-rejection view | 3-minute demo video | ❌ shell only (one static page) |

**P0 is not met** (#129). #4 was closed with P0 badges; the substantive
P0 story (gate thaw through `credit-gate` + SAS, an auction session, a
queue-claim sale, R5 on live NPCS) is still open and is graded here, not
badged. The per-criterion record is [`README.md` § P0 exit criteria](README.md#p0-exit-criteria);
the per-scenario grade is [SRS 08 §1.1](preresearch/architecture/08-SRS-Pilot-Rails.md#11--as-built-grade-2026-10).
The pitch must present the ❌ rows as roadmap.

**Guardrails from our own docs:** stay on devnet, keep NPCS inside team wallets (the offering gate in `08` §10.3), label everything `PILOT-DEMO-ASSET`, and never fake a mainnet markdown.

---

## 5 · How judges score it (rules §8), and our answer

| Criterion | Our answer |
|---|---|
| **Functionality / code quality** | Working devnet programs, property-tested `credit-math`, clean Anchor |
| **Potential impact / TAM** | $5B+ trapped behind gates, ~$8B tokenized private credit, 1–20% secondaries discounts (research `12`) |
| **Novelty** | Queue-claim: a pro-rated redemption request as a tokenized, financeable claim. Nobody has shipped this on any chain |
| **UX** | Denied redemption → exit or loan in one flow. Institutional dark UI |
| **Open-source / composability** | MIT license. Built on Token-2022 + USDC today; designed to compose with Token ACL, SAS, Kamino, Orca and RedStone (none integrated yet) |
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
