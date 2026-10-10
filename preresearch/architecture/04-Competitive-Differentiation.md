# Competitive Differentiation — How We Differ From Every Player in This Sector

> **⛔ Superseded (see `00-README.md`; current map in `../research/12`).** Integrations named below as if present — Kamino / Loopscale lending against our tokens, Orca pools for tranche tokens, an issuer SDK — are **roadmap**: no code in this repo calls those protocols. What is built is the mechanism layer (`credit-gate`, `auction-engine`, `mark-engine`, `queue-claim`, `liquidity-facility`, `tranche-engine`, `npcs-treasury`, `payment-rails`).

The sector's players grouped into four buckets — and exactly how we're different from each. No vague "we're better" claims; each difference is structural and verifiable.

---

## 1. The four buckets

```mermaid
flowchart TB
    subgraph B1["BUCKET 1 — Solana issuers (ACRED, SCOPE, PRIME, ONyc, USYC, BUIDL, VBILL)"]
        I1["tokenize credit, no secondary liquidity"]
    end
    subgraph B2["BUCKET 2 — Solana DeFi rails (Kamino, Loopscale)"]
        I2["lend against credit, no exit market"]
    end
    subgraph B3["BUCKET 3 — Solana-native credit attempts (Credix, Tranched.fi)"]
        I3["small, issuance/niche only"]
    end
    subgraph B4["BUCKET 4 — EVM benchmarks (Centrifuge, Maple, Goldfinch, TrueFi)"]
        I4["proven pattern, wrong stack"]
    end
    subgraph US["US"]
        U["Permissioned secondary market + tranching<br/>composable with all of the above"]
    end

    B1 -.-> US
    B2 -.-> US
    B3 -.-> US
    B4 -.-> US
```

---

## 2. Head-to-head differentiation

### vs. Solana issuers (ACRED, SCOPE, PRIME, ONyc, USYC, BUIDL)

```mermaid
flowchart LR
    subgraph THEM["Issuers"]
        T1["Issue permissioned credit tokens<br/>KYC whitelist, mint/redeem only"]
        T2["No secondary exit — holders<br/>wait for redemption windows"]
    end
    subgraph US2["Us"]
        U1["We never issue credit — we're the venue<br/>under the issuers"]
        U2["We give their tokens exit liquidity<br/>+ risk segmentation"]
        U3["We integrate them via issuer-sdk —<br/>they become our liquidity source"]
    end
    THEM -->|"complementary, not competing"| US2
```

**The structural difference:** issuers are *supply*; we're *distribution*. Their tokens become liquid *because of us*. This is the same relationship Kimia has with perps venues (it doesn't run the exchange AND tokenize — it tokenizes on top).

### vs. Kamino / Loopscale (Solana lending rails)

```mermaid
flowchart LR
    subgraph LEND["Kamino / Loopscale"]
        L1["Lend and borrow against RWA tokens<br/>(they consume collateral)"]
        L2["Cannot provide exit liquidity —<br/>lending ≠ trading venue"]
    end
    subgraph USL["Us"]
        U1["We provide the exit market —<br/>trading venue, not a lender"]
        U2["Our liquidity makes credit tokens<br/>BETTER collateral for them"]
        U3["We CPI-integrate: they lend, we trade"]
    end
    LEND -->|"flywheel partners"| USL
```

**The structural difference:** they're the *demand side* for credit collateral; we're the *liquidity side*. More secondary liquidity → better collateral → more lending. We make each other stronger; neither replaces the other.

### vs. Credix / Tranched.fi (the only Solana-native attempts)

```mermaid
flowchart LR
    subgraph CRED["Credix"]
        C1["Emerging-market debt pools only<br/>(single vertical)"]
    end
    subgraph TFI["Tranched.fi"]
        T1["Tranching primitives — early,<br/>not issuer-integrated"]
    end
    subgraph USC["Us"]
        U1["Cross-issuer: works with ACRED, SCOPE,<br/>PRIME, ONyc — any Token-2022 credit token"]
        U2["Both mechanisms: secondary market<br/>AND tranching, one composable stack"]
        U3["Issuer-sdk + hook-first compliance —<br/>institutional-grade from day one"]
    end
    CRED -.-> USC
    TFI -.-> USC
```

**The structural difference:** they're early verticals; we're the horizontal infrastructure layer. Credix is debt-pools-in-emerging-markets; Tranched.fi is tranching-only. We combine both mechanisms and integrate *every* issuer, not one vertical.

### vs. EVM benchmarks (Centrifuge, Maple, Goldfinch, TrueFi)

```mermaid
flowchart LR
    subgraph EVM["Centrifuge / Maple / Goldfinch / TrueFi (EVM)"]
        E1["Proven issuance + permissioned pools<br/>+ transfer-agent compliance"]
        E2["Compliance = ERC-3643 identity<br/>bolt-ons + transfer agents"]
        E3["Fragmented across EVM chains —<br/>composability with Solana RWA rails = 0"]
    end
    subgraph USV["Us"]
        U1["Same proven pattern — but native to Solana<br/>Token-2022 hooks in the token runtime"]
        U2["Compliance is a property of the token,<br/>not an external contract layer"]
        U3["Composes with Kamino/Loopscale —<br/>the RWA rails EVM players can't touch"]
    end
    EVM -->|"we port the pattern, fix the stack"| USV
```

**The structural difference:** they proved the *pattern* (permissioned pools + tranching + transfer agents work); we implement it where the *native compliance machinery and RWA liquidity* actually live. We're not reinventing Centrifuge — we're building its Solana-native successor with the token-layer compliance EVM can't offer.

---

## 3. The differentiation summary matrix

| Player | What they do | What we do instead / better | Why we win |
|---|---|---|---|
| ACRED, SCOPE, PRIME, ONyc, USYC, BUIDL, VBILL | Issue permissioned credit tokens | Give them secondary liquidity + tranching | They're supply; we're distribution. No conflict, pure synergy |
| Kamino, Loopscale | Lend against RWA tokens | Provide the exit market | They're demand; we're liquidity. Flywheel, not fight |
| Credix | EM debt pools (Solana) | Cross-issuer, both mechanisms | Horizontal vs. vertical |
| Tranched.fi | Tranching primitives (early) | Tranching + market + issuer SDK | Full stack, issuer-integrated |
| Centrifuge, Maple, Goldfinch, TrueFi | EVM permissioned credit | Same pattern, native Solana | Token-2022 compliance in the runtime; composes with Kamino/Loopscale |
| Open DEXes (Raydium/Meteora) | Open markets | Gated markets for restricted assets | They legally can't host these; we can |

---

## 4. The moat (why we can't be copied overnight)

```mermaid
flowchart TD
    M1["First-mover issuer integrations<br/>(ACRED/PRIME/ONyc SDKs live)"] --> MOAT["MOAT"]
    M2["Hook-first compliance architecture<br/>(regulatory trust = slow to replicate)"] --> MOAT
    M3["Network effects: liquidity ↔ collateral loops<br/>(Kamino/Loopscale integration)"] --> MOAT
    M4["Composability = every new credit issuer<br/>automatically gains a market"] --> MOAT
```

---

## 5. What we are NOT (to keep the pitch honest)

- ❌ Not an issuer — we don't originate or tokenize credit.
- ❌ Not a lender — we don't lend capital.
- ❌ Not an open retail market — we're accredited-only by design.
- ❌ Not a fork of Centrifuge — we're a native-Solana implementation of the proven pattern.
- ❌ Not competing with Helius/Light privacy layer — orthogonal (they do confidentiality; we do credit market infrastructure).

---

*Back to `00-README.md` for the folder index, or `03-Feasibility-and-Compliance.md` for the why-Solana case.*
