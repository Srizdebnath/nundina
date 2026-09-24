# NUNDINA — Market Research & 2026 Reality Check

> **Fills the gap every architecture doc points to:** `architecture/00-README.md` names this as the start-here validation doc; it previously did not exist. Compiled **24 September 2026** from a ~20-angle web research sweep (sources inline).
>
> **Product:** NUNDINA — the pricing, queue and risk-transfer engine for gated, NAV-priced private credit on Solana. v1 = issuer-sponsored liquidity window. Builds on `architecture/05-Problem-Register.md` and `architecture/06-Solution-Architecture-v2.md`.

---

## 1. Executive summary — what changed, what holds

**The problem register (May–June 2026 vintage) is still directionally correct on mechanism, but four material things have changed by late September 2026:**

1. **The gate crisis is now front-page news, at scale.** Apollo capped redemptions at its flagship retail private credit fund after requests hit **17% of NAV** — more than 3× the 5% cap (CNBC, June 2026; Reuters, Sept 22, 2026). Withdrawal requests across large funds surged **+$4.3B QoQ (+56%)**. Blue Owl, Blackstone, BlackRock, Cliffwater funds all hit caps. **~$5B+ is trapped behind gates.** This is the exact population NUNDINA's `queue-claim` serves, and the number is growing quarterly. The thesis — "the gate, not the lock-up, is the binding constraint" — is no longer a hypothesis; it is the market's defining event.
2. **The SEC Innovation Exemption LANDED — September 17, 2026.** The problem register (C17) says "delayed 26 May 2026 after exchange-lobby pushback, architecting on it is architecting on a press release." That is now **stale**. The SEC granted **five-year conditional exemptive relief** to "Tokenized Securities Venues" (TSVs) using permissioned AMM liquidity pools. But read the scope carefully: it covers **tokenized NMS stock** (public equities), *not* Reg D private fund shares. It is validation of the permissioned-AMM venue model, and a template — not yet a license for NUNDINA's core asset class.
3. **The venue layer is real and crowded (as predicted).** Securitize+Jump+Jupiter shipped regulated onchain trading (May 5); Orca shipped permissioned pools (May 27); Exponent shipped risk tranching (June 24); Uniswap Labs opened permissioned pools for tokenized funds (July 23). NUNDINA's "be the mechanism, not the venue" reframe survives this — and is *strengthened* by the fact that every one of these venues still has no answer for pricing a stale-NAV gated asset.
4. **A TradFi-native liquidity competitor emerged: Midas.** $50M Series A (March 2026) plus a **$40M liquidity facility** buying/supplying liquidity against tokenized assets with instant-redemption mechanics. That is a direct attack on the `liquidity-facility` business line, funded 50–100× a typical grant-funded startup. It validates the model commercially; it also means NUNDINA should not plan to win on balance-sheet size.

**Net thesis verdict:** the wedge (issuer-sponsored liquidity windows + queue-claims) is **more urgent than when the docs were written**, the mechanism (auction + mark engine) remains unbuilt by anyone, and the single biggest legal blocker (C1, venue registration) now has a credible federal template to point to — even though the current order does not cover private funds yet.

---

## 2. Market size & growth

| Metric | Value | Source / date |
|---|---|---|
| Tokenized RWA on-chain (ex-stablecoins) | **$33.5B** (July 2026) | rwa.xyz via Stobox "State of RWA 2026" |
| Tokenized RWA AUM | **$22B+** (May 2026), ~75% YoY | Eco / rwa.xyz, Sep 2026 |
| Tokenized private credit on-chain | **~$8B**, second-largest RWA class after Treasuries; growing ~74% YoY; most DeFi-composed RWA category (64.3% of market by composability) | rwa.xyz, Pantera "State of Tokenization Q1 2026" |
| Tokenized private credit broader figure | **$18.9B** cited for the vertical (April 2026) | Solana RWA ecosystem recap (LinkedIn/Xangle) |
| Solana non-stablecoin RWA | **$3.7B across 313K holders** (late July 2026); ~$4B milestone later cited | solana.com "Overview of Institutional RWA on Solana," Jul 30 2026 |
| Solana tokenized-asset spot volume | **$5.77B in Q2 2026** (quarterly ATH) | Yahoo Finance / Sam, Jul 2026 |
| TradFi private credit AUM | **~$2T**, with $16.2B of redemptions outstanding | MarketWatch / CNBC 2026 |
| Traditional secondaries volume (all private markets) | Record **$240B in 2025**, credit secondaries a fast-growing share | Masterworks / William Blair 2026 report |
| Credit secondaries pricing | LP-led trades price at **1–20% discounts to NAV** (S&P); buyout mid-single-digit discounts; PE avg ~87% of NAV in 2025 | S&P Global Ratings Sep 2026; Commonfund Feb 2026 |

**Read for NUNDINA:** the fee-relevant pool is not "$2T private credit"; it is the **gated, tokenized slice**: on-chain private credit float on Solana is small (§3), but the demand side (denied redemptions) is measured in **billions per quarter**, is growing, and every denied dollar is a potential queue-claim origination. A 1% fee on $5B of trapped quarterly demand is a $200M/yr revenue pool *if* the flow can be legally captured — the sequencing risk is regulatory, not demand-side.

---

## 3. Solana ecosystem readiness — all the rails now exist

- **Token ACL (sRFC37)** — live and documented by the Solana Foundation: default-frozen accounts, permissionless thaw via pluggable Gate Programs, zero-CU transfer path. Official docs + spec on GitHub. This is exactly what `credit-gate` builds on. (solana.com/docs/tokenization/token-acl; solana-foundation/token-acl srfc37.md)
- **Solana Attestation Service (SAS)** — live since May 2025, in production use by identity/credential providers. Supports `credit-gate`'s eligibility checks. (solana.com/news/solana-attestation-service)
- **Orca permissioned pools** — launched **May 27, 2026** for regulated tokenized assets, access control at token level. The continuous-venue integration target for tranche tokens exists. (CoinDesk)
- **Securitize + Jump + Jupiter** — regulated onchain trading stack on Solana (May 5, 2026) for tokenized equities; Securitize's ATS + transfer agent + Solana deployment. (PR Newswire)
- **Lending rails:** Kamino (~$2.1–2.4B TVL, RWA markets incl. ACRED; PRIME lending market >$350M TVL) and Loopscale (order-book lending; **~95% of all ACRED minted on Solana sat on Loopscale by end Q1 2026**; later added Neuberger HINC as collateral). (Kamino, Loopscale blog, The Defiant)
- **NAV feeds:** RedStone x Securitize feeds live (ACRED NAV on-chain), NAVLink/SmartNAV (Chainlink) available; issuer-attested NAV as fallback (OnRe publishes monthly NAV attestations). Problem B4 has a real answer for pilot assets.
- **Custody:** Anchorage Digital Bank supports SPL tokens; Fireblocks supports Solana + Token-2022 permissioned templates across 35+ chains; both are institutional shortlist custodians. Launch-gate condition (F7) is satisfiable today. (Anchorage press, Fireblocks)
- **Grants/RTM:** Solana Foundation funding programs active (rolling applications); a **$250M Catalytic Fund dedicated to RWA** has been referenced in ecosystem summaries; Foundation publishes institutional-RWA narrative support. K6 (grant framing) remains open-core-friendly: Gate Program + auction engine + public discount dataset.

**Deliberate-omissions check:** still correct. No AMM (Orca), no credential registry (SAS), no NAV oracle (RedStone/NAVLink/attestations), no issuance platform (Securitize), no custody (Anchorage/Fireblocks/Kamino's Anchorage tri-party). Nothing in the ecosystem has moved toward building NUNDINA's six programs.

---

## 4. Competitive landscape — mapped against NUNDINA's five primitives

### 4.1 On Solana (direct adjacency)

| Player | What they shipped (2026) | What they do NOT do (NUNDINA's lane) |
|---|---|---|
| **Securitize (+Jump, Jupiter)** | ATS-regulated trading, transfer agency, issuance; regulated onchain trading stack | No periodic price discovery for stale-NAV assets; no queue-claim; no mark engine. Could build it (I6 risk) — engine-licensing posture is the defense |
| **Orca** | Permissioned pools (CLMM) for regulated tokens (May 27) | A pool is exactly the wrong primitive for raw fund shares (A1 adverse selection). They host *tranche tokens*; they don't price the raw asset |
| **Exponent Finance** | v2 (May 2026); risk tranching live (June 24) on ONyc: 6.4% protected senior / 31.4% amplified junior; $1M+ tranching suite | Yield-variance tranching on permissionless yield tokens — not credit-loss waterfall on gated fund shares (E1–E12 gap stands). **Action: read Exponent's tranche accounting; it is the closest prior art to §6** |
| **Kamino / Loopscale** | RWA lending markets; ACRED as collateral at scale; HINC collateral | Lending *against* gated collateral with the liquidation problem unsolved (F8) — they cannot dump gated collateral on a DEX; NUNDINA's facility + forced-sale auction answers it |
| **OnRe (ONyc)** | $300M AUM tokenized reinsurance yield; on-chain price up 3.4% Jun→Sep; Season 1 rewards | An issuer, not a competitor. **Most realistic first sponsored-window issuer** (already runs attestation publishing, liquidity-oriented, less restrictive eligibility). Also Exponent's launch asset — competition for junior capital exists here |
| **Figure / Hastra (YLDS, wYLDS, PRIME)** | SEC-registered yield token across Provenance/Solana/Stellar; PRIME market on Kamino >$350M | Yield-token distribution, not a pricing/queue mechanism. PRIME/wYLDS are semi-permissioned wedge assets per K3 sequencing |

### 4.2 TradFi & cross-chain

- **Midas** — the most direct commercial threat: **$50M Series A (Mar 2026) + $40M liquidity facility** for instant liquidity on tokenized assets; Fasanara private credit brought onchain via Midas and integrated with Kamino RWA USDC markets. Validates that "liquidity layer for tokenized funds" is a fundable, sizable business. NUNDINA's counter: auction mechanism (no passive LP to pick off), queue-claims (they have none), and issuer-sponsorship posture vs. Midas's balance-sheet posture.
- **Forge Global / NPM / EquityZen / Hiive** — TradFi secondaries rail. Forge charges **3–6% per trade** (a devastating data point *for* the "revenue is not trading fees" thesis — compliance-heavy venues need high take rates that gated credit flow will not bear); NPM is pushing into fund secondaries. None are on-chain, none tokenize the queue.
- **Centrifuge** — tokenized credit infrastructure (Coinbase backbone, Morpho markets, Caesar equity tokenization); a *credit* stack on Ethereum/Base rather than a secondary-pricing mechanism for third-party gated funds. Adjacent, partner-shaped.
- **Uniswap Labs** — permissioned pool standard for tokenized funds (July 23, 2026): confirms permissioned AMMs are becoming commodity infrastructure. Strengthens "the venue is not the moat."

### 4.3 Whitespace verdict

Nobody — on any chain or off — has shipped: **(a)** periodic price discovery designed for stale-NAV assets (sealed-bid batch auction), **(b) a tokenized redemption-queue claim**, **(c) a credit-loss waterfall on gated fund shares with realized/unrealized loss accounting**, **(d) liquidation for permissioned collateral.** The May–June 2026 launches filled the *venue* layer NUNDINA chose not to build. The mechanism layer is still empty.

---

## 5. Demand-side evidence — the gate crisis in numbers

- **Apollo (ACRED's sponsor ecosystem):** capped redemptions in its main retail-focused private credit fund after requests hit **17% of NAV** — >3× the 5% cap (CNBC, Jun 23, 2026). Reuters (Sept 22, 2026): most non-traded private credit funds enforced the 5% limit this year; requests remained elevated even as Apollo's eased in Q3.
- **Aggregate:** withdrawal requests from large private credit funds surged **+$4.3B QoQ (+56%)**; **~$5B+ waiting behind gates** (AdvisorHub, Mar 2026); MarketWatch: "another redemption wave is spooking the $2T private credit market."
- **Fitch:** private credit default rate approached **~10% mid-2025** and climbing into 2026; CNBC (Mar 2026): "private credit's zero-loss fantasy is ending."
- **First Brands & Tricolor** bankruptcies (late 2025) triggered the confidence shock; funds gated *after* mark-to-market stress became visible. CAIA (Apr 2026): "gates have gone up" across semi-liquid vehicles.
- **Pricing consequence:** credit secondaries clear at **1–20% discounts to NAV** (S&P Global, Sep 2026) — this is the observable discount band NUNDINA's auctions would formalize on-chain. Stressed/short-duration paper trades at the wide end; this is where the auction print has immediate utility.

**Implication for A5/A12:** the "issuer's redemption window is the competing venue" objection is *weakening in practice* — with 3× oversubscribed gates, the window itself is rationed, and the denied residual is NUNDINA's customer base. Issuers now have an active PR problem (CNBC headlines) that a liquidity window solves; H2's "pay the issuer" pitch writes itself: gate relief + NAV-accretive buybacks + controlled signaling.

---

## 6. Regulatory — the September 17, 2026 SEC order (updates C17, partially de-risks C1)

**What the order does** (SEC Press Release 2026-90, Sept 17, 2026; Sullivan & Cromwell, Dechert memos):
- Grants **five-year conditional exemptive relief** to "Tokenized Securities Venues (TSVs)" from the Exchange Act "exchange" definition, for trading **tokenized NMS stock** via permissioned AMM liquidity pools.
- Also exempts liquidity providers in TSV AMM pools from the "dealer" definition (conditional).
- Conditions: symbol/volume limits; token must confer identical rights; notice+objection right to the underlying issuer; **auditable public smart contracts on a public permissionless ledger**; trading halt parity; public operating transparency.

**What it does NOT do:**
- Covers **NMS stock only** — not Reg D 506(b)/(c) fund shares, which are NUNDINA's v1 assets. C1 (venue registration / Rule 3b-16) still applies to a third-party matching venue for private fund interests **today**.
- Does not touch Rule 144 per-lot seasoning (C2), 12(g) holder counts (C4), broker-dealer compensation (C3) — those are statutory/regulatory, unchanged.

**Strategic read:**
1. The issuer-sponsored window (v1) does **not** need the exemption — an issuer running repurchase auctions in its own securities was already a different regulatory object. The order *adds* a second door: a TSV-style venue could later be the regulated shell for third-party flow, with auditable public smart contracts (NUNDINA programs qualify structurally) — **treat as upside, still not load-bearing** (C17 discipline holds, but flip from "delayed" to "template exists").
2. Comment period + Atkins/Selway statements signal more scope expansion is plausible within the 5-year window. A comment letter arguing extension of TSV relief to **permissioned Reg D secondary venues** would be cheap, public-good positioning (also good for K6 grant framing).
3. EU: MiFID II routing requirement unchanged (C7). Non-US/Reg S flow remains the v1 sanity path per §9.

---

## 7. Business model & revenue benchmarks

| Comp | Take rate | Implication for NUNDINA |
|---|---|---|
| Forge Global | **3–6% per trade** | Full-service compliance venues need fees 100–300× above the 10–25bps trading-fee model the problem register already rejected (A8). Confirms: do not monetize trading |
| TradFi secondaries (S&P) | Discounts 1–20% of NAV on LP trades | The spread between auction clear and NAV is the fee room: a facility advancing at 50–70% PV of a claim worth 85–95% of NAV has a wide, defensible spread |
| NAV lending (KBRA) | Record 2025 issuance, mainstream 2026; typically SOFR+ few-hundred-bps fund-level | `liquidity-facility` is the on-chain version of a $100B+ TradFi product going mainstream — but LP-level, which TradFi barely serves. KBRA rating culture implies: marks must be reproducible (J1) or no institutional lender scales |
| Midas | Raised $50M; $40M facility deployed against tokenized assets | Liquidity provision is venture-fundable at scale; NUNDINA should route to Kamino/Loopscale where possible (already §5 routing) and treat own balance sheet as last-resort |
| Exponent | $5M raise (Multicoin); capped junior ($2.5M alpha cap cited in problem register) | Junior capital remains the binding constraint on tranching (E1); sponsor-retained first loss stays the default |

**Refined revenue stack (v1):** (1) issuer sponsorship/software license for the liquidity window; (2) facility spread + origination on queue-claim advances; (3) data product: the discount-curve print set for gated assets (nobody has this dataset; Forge sells data as a core line). No trading fees (A8/C3 hold).

---

## 8. Risks & open items (what this research did NOT resolve)

1. **K1/H1 issuer consent** — still the gating item. Positive: gate-crisis headlines give issuers a *reason to say yes* for the first time. Negative: no named issuer has committed. Next step is issuer pipeline (OnRe most plausible first).
2. **C1 scope** — the TSV order is equities-only; private-fund venue relief is a matter of time, not certainty. Reg S / non-US first remains prudent; issuer-sponsored window remains the domestic wedge.
3. **$10M float problem (K2)** — Solana on-chain private-credit float is still small (ACRED ~$115M fund, large share on Loopscale; ONyc ~$160M+ on-chain; STAC expanded to Solana June 2026 with Ethena's $250M intent). The float is growing fast but NUNDINA's year-1 fee base is constrained; sell sponsorship + data, not volume fees.
4. **Midas well-funded overlap** — if Midas extends from instant-liquidity facility into gated-fund windows, the queue-claim wedge could be contested. Move fast on one issuer.
5. **Cycle risk (M1/M2)** — default rate ~10% and rising; the first on-chain credit-loss waterfall will likely live through markdowns. Design-for-down-case (three-book accounting) is not optional.
6. **Junior capital (E1)** — no evidence anyone solved institutional junior demand for gated-credit tranches; Exponent's model caps alpha. Sponsor-retained first loss remains the v1 default.
7. **Unverified figures** — market-size numbers vary by source ($8B vs $18.9B private credit; $22B vs $33.5B RWA total) depending on methodology and date; treat as order-of-magnitude. TheApollo 17% and SEC order facts are primary-sourced.

---

## 9. Bottom line

The 2026 reality check **strengthens** the NUNDINA thesis on all three axes that matter:

- **Problem urgency:** the gate crisis is the defining event of private credit 2026 ($5B+ trapped, Apollo at 17% request/5% cap, defaults ~10%). The queue-claim wedge targets the exact wound the market is bleeding from.
- **Whitespace:** venues shipped; mechanisms didn't. No passive-LP-free price discovery, no queue-claim, no gated-collateral liquidation path exists anywhere.
- **Legal trajectory:** first federal template for permissioned on-chain venues exists (TSV order, Sept 17, 2026), validating the architecture's compliance-at-activation design (Token ACL + SAS) even though private funds are not yet in scope.

The bottleneck is unchanged and unchanged in kind: **get one issuer to sponsor a window** (K1/H1). Everything this research found — Apollo's crisis, OnRe's attestation publishing, Kamino/Loopscale's collateral appetite, the SEC's template — lowers the cost of that first yes.

---

## Appendix A — Key sources

- SEC Press Release 2026-90 + Atkins & Uyeda statements (Sept 17, 2026); Sullivan & Cromwell & Dechert client memos
- CNBC: Apollo caps redemptions at 17% request level (Jun 23, 2026); Reuters: Apollo Q3 easing, 5% limits industry-wide (Sep 22, 2026); WSJ (Mar 2026); AdvisorHub ($5B trapped, Mar 2026); MarketWatch redemption wave
- CAIA (Apr 2026) gates analysis; WealthManagement.com semi-liquid redemption crisis (Mar 2026); Fitch default data via MarketWatch
- rwa.xyz via Stobox State of RWA 2026 ($33.5B), Eco RWA market size (May 2026), Pantera State of Tokenization Q1 2026
- Solana Foundation: "Overview of Institutional RWA on Solana" (Jul 30, 2026), Token ACL docs (sRFC37), SAS announcement, ecosystem roundups
- CoinDesk: Orca permissioned pools (May 27, 2026); PR Newswire: Securitize/Jump/Jupiter (May 5, 2026), STAC to Solana (Jun 12, 2026)
- SolanaCompass: Exponent risk tranching on ONyc (Jun 26, 2026); OnRe $300M AUM (Sep 2026); Loopscale project review
- Loopscale blog: Q1 2026 recap (95% of Solana ACRED), ACRED/USDG loop ($6.855M)
- The Block / TNW: Midas $50M Series A + $40M liquidity facility (Mar 30, 2026); vestbee
- S&P Global Ratings: secondaries discounts 1–20% (Sep 9, 2026); Commonfund LP-led pricing (Feb 2026); William Blair 2026 Secondary Market Report; Masterworks ($240B 2025)
- Forge Global financials/fee structure (BusinessWire, Moneymade); NPM fund secondaries push (BriefGlance, Jul 2026)
- KBRA NAV lending report (Jul 2026); Moonfare/Mayer Brown NAV facility primers
- Kamino ($2.1B+ TVL, PRIME >$350M), The Defiant (HINC collateral, Sep 2026), Xangle Solana RWA series (Jun 2026)
- Figure/Hastra YLDS/wYLDS/PRIME releases; Stellar YLDS launch (May 5, 2026)
- Anchorage Digital SPL custody; Fireblocks Solana/Token-2022 support
- CNBC private credit zero-loss fantasy (Mar 25, 2026); Cambridge Associates & Man Group on First Brands/Tricolor
