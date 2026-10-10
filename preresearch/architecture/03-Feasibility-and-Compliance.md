# Feasibility & Compliance — Why This Is Only Possible on Solana

> **⛔ Superseded (see `00-README.md`).** Kept as the original why-Solana case. As built, compliance is **not** a Token-2022 Transfer Hook: NPCS is default-frozen and thaws only through Token ACL (sRFC37) → `credit-gate`'s `validate`, which reads SAS attestations at account activation, and every fill is re-checked by `credit-gate` `settle_check` inside payment-rails' `settle_fill` (`06` §3, `DEPLOYMENTS.md`). The Transfer Hook diagrams below describe the design that was rejected.

The "why Solana," "why now," and "why this design" case — plus the compliance enforcement detail that makes a permissioned secondary market *natively expressible* instead of bolted-on.

---

## 1. Why this is "only possible on Solana" (the grant criterion)

```mermaid
flowchart TB
    subgraph WHY["Why Solana — native advantages"]
        W1["Token-2022 Transfer Hooks<br/>compliance enforced IN the token runtime<br/>(CPI on every transfer)"]
        W2["Permanent Delegate<br/>issuer freeze/seize/reverse —<br/>legally required control"]
        W3["Sub-second finality + sub-cent fees<br/>fine-grained matching, small tickets,<br/>frequent auctions"]
        W4["Existing RWA DeFi rails<br/>Kamino ~87% of RWA DeFi TVL,<br/>$620M+ deposits — demand exists"]
        W5["Atomic composability<br/>credit tokens → collateral loops<br/>in one transaction"]
    end

    subgraph WHYNOT["Why NOT EVM (the comparison)"]
        E1["ERC-3643 identity bolt-on<br/>compliance outside the token"]
        E2["High gas → illiquid small tickets<br/>permissioned pools fragment"]
        E3["No native permanent-delegate<br/>equivalent at standard level"]
    end

    WHY --> CONC["Permissioned secondary market<br/>that is still composable + liquid"]
    WHYNOT --> CONC
```

**The one-sentence argument for the grant:** *"On EVM, a compliant secondary market for securities is a fragile assembly of identity contracts and transfer agents; on Solana, Token-2022 makes accredited-only transfers a property of the token itself — so we get a permissioned market that remains natively composable with lending rails. Only possible on Solana."*

---

## 2. Why now (timing evidence)

```mermaid
flowchart LR
    A["$14–18B tokenized private credit<br/>(largest RWA class)"] --> D["locked in static vaults,<br/>zero secondary exit"]
    B["$1.6–3.2T traditional private credit<br/>allocators cite liquidity as #1 blocker"] --> D
    C["Foundation mandate shift:<br/>issuance → institutional liquidity<br/>& financial utility"] --> D
    D --> E["The moment for a<br/>secondary market + tranching layer"]
```

---

## 3. Why this design (the mechanisms exist to be built on)

```mermaid
flowchart TD
    A["Tokenized credit exists but is illiquid"] --> B["Mechanism 1: Permissioned AMM + orderbook<br/>provides exit & entry without issuer redemption"]
    A --> C["Mechanism 2: Senior/Junior tranching<br/>segments risk for institutional appetites"]
    B --> D["Composable with Kamino/Loopscale<br/>→ credit becomes borrowable collateral"]
    C --> D
    D --> E["Foundation's stated priority #1<br/>(institutional adoption + RWA)"]
```

---

## 4. Compliance enforcement detail (the load-bearing design)

### The enforcement flow on every transfer

```mermaid
sequenceDiagram
    participant Tx as Any Transfer (AMM/orderbook/transfer)
    participant Hook as Token-2022 Transfer Hook (CPI)
    participant Reg as credential-registry
    participant SDL as Sanctions / Limits Check
    participant ISS as Issuer Permanent Delegate

    Tx->>Hook: SPL transfer initiated
    Hook->>Reg: CPI: is sender accredited?
    Hook->>Reg: CPI: is receiver accredited?
    Hook->>SDL: CPI: sanctions / holder-count / lock-up checks
    alt all checks pass
        SDL-->>Hook: pass
        Hook-->>Tx: allow transfer (atomic)
    else any check fails
        SDL-->>Hook: fail
        Hook-->>Tx: revert entire instruction
        Note over Hook,ISS: zero state change — compliance is atomic,<br/>not a separate settlement step
    end
    Note over ISS: permanent delegate retained as issuer's<br/>emergency authority (court orders, lost keys)
```

### The four legal guardrails the design respects

```mermaid
flowchart LR
    G1["Reg D / 506(c)<br/>accredited-only purchase<br/>+ exempt secondary sales"] --> C["COMPLIANT BY CONSTRUCTION"]
    G2["2,000-holder rule (12g)<br/>investor-ID aggregation"] --> C
    G3["Transfer agent mandate<br/>off-chain master file sync"] --> C
    G4["MiCA / MiFID II / GENIUS Act<br/>regional eligibility routing"] --> C
    C --> H["Hooks + delegate + registry<br/>make these enforceable in the token layer"]
```

---

## 5. Feasibility summary — what's proven vs. what we build

| Capability | Status on Solana | Our role |
|---|---|---|
| Token-2022 hooks + delegate | ✅ Live (used by ACRED, USYC, BUIDL) | We build on it |
| High-speed orderbooks (Phoenix/OpenBook) | ✅ Live | We wrap them with hooks |
| AMMs (Raydium/Meteora) | ✅ Live | Gated variants |
| Lending against RWA tokens | ✅ Live (Kamino, Loopscale) | We feed them liquid collateral |
| **Permissioned secondary market for credit** | ❌ **Does not exist** | **We build it** |
| **Senior/junior tranching for credit** | ❌ **Nascent (Credix, Tranched.fi — tiny)** | **We build it** |
| Credential/accredited registry | ⚠️ Fragmented (Civic, partner solutions) | We standardize it for credit |

**Verdict: 100% feasible — the hard parts (native compliance, fast settlement, lending rails) are already live; the missing pieces are exactly the two mechanisms we're building.**

---

## 6. Risk register (honest)

| Risk | Mitigation |
|---|---|
| Regulatory change (holder rules, accreditation) | Investor-ID aggregation, regional routing, issuer delegate as override |
| Cold-start liquidity | Seed with PRIME/ONyc (already secondary-friendly), incentive LPs, pair with Kamino |
| NAV/oracle manipulation | Staleness guards, multiple providers, confidence bands, mark-to-model fallback |
| Smart-contract risk | Separate auditable programs (one responsibility each), round-against-trader math |
| Helius/canonical-layer competition | We're complementary: they build privacy/compression; we build credit market infrastructure |

---

*Next: `04-Competitive-Differentiation.md` — exactly how we differ from every player in the sector.*
