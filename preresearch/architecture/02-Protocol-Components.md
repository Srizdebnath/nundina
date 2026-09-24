# Protocol Components & Transaction Flows

The on-chain program architecture — every program, its responsibilities, its accounts, and the critical flows as sequence diagrams. Modeled on Kimia's proven six-program composable stack.

---

## 1. Program map (six Anchor programs, one fixed-income-market stack)

```mermaid
flowchart TB
    subgraph PROGS["OUR SIX PROGRAMS"]
        P1["credit-amm<br/>Permissioned AMM<br/>x·y=k, hooks-gated"]
        P2["credit-book<br/>Permissioned orderbook<br/>Phoenix/OpenBook-style"]
        P3["tranch-engine<br/>Senior/Junior waterfall<br/>share-class tokens"]
        P4["credential-registry<br/>Accredited/KYC credential<br/>non-transferable proofs"]
        P5["nav-oracle<br/>NAV feed + mark-to-model<br/>staleness guards"]
        P6["issuer-sdk<br/>Deploy + whitelist + hooks<br/>integration layer"]
    end

    P1 --> P4
    P2 --> P4
    P3 --> P4
    P1 --> P5
    P2 --> P5
    P6 --> P1
    P6 --> P2
    P6 --> P3
```

**Design principles (borrowed from Kimia's playbook, adapted for credit):**
- **One program, one responsibility** — matching ≠ tranching ≠ compliance; each independently auditable.
- **Permissionless cranks, gated by hooks** — keepers can trigger rebalances/liquidations, but *every token movement* still passes the compliance hook.
- **Round-against-trader** — shared math crate (`credit-math`) always rounds against the taker.
- **Compliance by construction** — the registry is *read* by hooks; a wallet without a valid credential cannot even enter the order flow.

---

## 2. Account model per program

```mermaid
flowchart LR
    subgraph CA["credit-amm accounts"]
        CA1["Market<br/>mint pair · hook program · fee vault"]
        CA2["LiquidityPosition<br/>per-provider LP shares"]
        CA3["Order<br/>side · price · qty · reserved"]
    end

    subgraph TA["tranch-engine accounts"]
        TA1["Pool<br/>collateral · senior/junior mints"]
        TA2["Tranche<br/>waterfall params · cap · status"]
        TA3["InvestorPosition<br/>per-holder senior & junior balances"]
    end

    subgraph RA["credential-registry accounts"]
        RA1["Credential<br/>holder · issuer · expiry · attester"]
        RA2["AttesterSet<br/>approved KYC providers"]
    end

    subgraph OA["nav-oracle accounts"]
        OA1["Feed<br/>asset · latest NAV · staleness ts"]
        OA2["Provider<br/>authorized NAV publisher"]
    end

    CA1 --> RA1
    CA2 --> RA1
    TA1 --> RA1
```

---

## 3. Transaction flows (the critical paths)

### Flow 1 — Investor onboarding (one-time)

```mermaid
sequenceDiagram
    participant Inv as Investor (wallet)
    participant KYC as KYC Provider (Civic/partner)
    participant Reg as credential-registry
    participant Hook as transfer hooks (all markets)

    Inv->>KYC: complete KYC + accredited check (off-chain)
    KYC-->>Inv: signed attestation
    Inv->>Reg: register_credential(attestation)
    Reg-->>Inv: non-transferable Credential PDA
    Note over Reg,Hook: from now on, hooks recognize this wallet<br/>as eligible for all gated markets
    Hook->>Reg: verify eligibility on every transfer
```

### Flow 2 — Secondary trade (the core loop)

```mermaid
sequenceDiagram
    participant S as Seller (credit holder)
    participant H as transfer-hook program
    participant M as credit-amm / credit-book
    participant B as Buyer (accredited)
    participant O as nav-oracle
    participant R as credential-registry

    S->>M: place sell (credit tokens, price)
    B->>M: place buy (USDC, price)
    M->>O: get current NAV-based mark
    M->>H: CPI validate(buyer)
    H->>R: buyer has valid credential?
    R-->>H: yes
    H->>H: check sanctions/limits
    H-->>M: approved
    M->>M: atomic swap + fee split (taker/maker + insurance skim)
    M-->>S: USDC
    M-->>B: credit tokens
    Note over M,H: ineligible buyer → revert, zero state change, no gas waste beyond CPI cost
```

### Flow 3 — Tranching lifecycle

```mermaid
sequenceDiagram
    participant Mgr as Pool Manager / Issuer
    participant TE as tranch-engine
    participant Pool as Collateral Pool
    participant Sr as Senior Holders
    participant Jr as Junior Holders
    participant IF as Reserve Buffer

    Mgr->>TE: create_pool(collateral, senior_cap, junior_cap)
    Mgr->>TE: deposit credit collateral
    TE->>Sr: mint senior tokens (claim 1st)
    TE->>Jr: mint junior tokens (claim residual)
    loop each cash-flow period
        Pool->>TE: cash flows in
        TE->>Sr: pay senior coupons first
        TE->>IF: skim reserve
        TE->>Jr: pay residual
    end
    Note over TE,Jr: losses: junior absorbs first → senior shielded<br/>this risk segmentation is the institutional ask
```

### Flow 4 — Liquidation / bad-debt path

```mermaid
sequenceDiagram
    participant K as Keeper (anyone)
    participant M as credit-amm
    participant TE as tranch-engine
    participant IF as Reserve Buffer
    participant ISS as Issuer (permanent delegate)

    K->>M: detect underwater position (NAV < threshold)
    M->>TE: trigger liquidation (hooks-gated)
    TE->>IF: draw reserve buffer first
    TE->>ISS: if still short → issuer freeze + recovery
    Note over TE,ISS: junior absorbs before reserve; issuer delegate is the final<br/>safety net (court orders, sanctions, lost keys)
```

---

## 4. Integration with existing Solana rails

```mermaid
flowchart LR
    subgraph OUR["Our stack"]
        AMM["credit-amm"]
        BOOK["credit-book"]
        TR["tranch-engine"]
    end

    subgraph SOL["Existing Solana DeFi"]
        KAM["Kamino RWA Markets<br/>(lending, looping)"]
        LP["Loopscale<br/>(order-book lending)"]
        STE["Steakhouse<br/>(risk curation)"]
    end

    AMM --> KAM
    BOOK --> LP
    TR --> STE
    AMM -->|transfer-hook CPI| COMP["credential-registry"]
    BOOK -->|transfer-hook CPI| COMP
    TR -->|transfer-hook CPI| COMP
```

**Composability story:** credit tokens that gain real secondary liquidity become *better collateral* — Kamino and Loopscale can lend against tranche tokens; Steakhouse curates the risk parameters. Our layer makes the whole RWA DeFi flywheel more liquid, which is exactly the Foundation's "issuance → utility" mandate.

---

*Next: `03-Feasibility-and-Compliance.md` — why this is only possible on Solana, and the compliance enforcement detail.*
