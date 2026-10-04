# NPCS — Token Architecture

How the pilot token is built on Solana, what every byte of it does, and where
it is going. Companion to `TOKENOMICS.md` (the money rules) and `runbook.md`
(the ops). Spec authority: `../preresearch/architecture/09`.

---

## 1 · Mint anatomy — the extension manifest

NPCS is a Token-2022 mint with an exact set of extensions. Everything the
token is, permission-wise, is decided by this manifest — asserted on-chain by
`scripts/verify.ts` on every run:

```mermaid
flowchart TB
    subgraph MINT["NPCS mint account (Token-2022, 581 bytes)"]
        direction TB
        BASE["Base mint state (82B)<br/>decimals = 6 · supply = deposits ·<br/>mint authority · freeze authority"]
        subgraph TLV["Extension TLV region (from byte 166)"]
            DAS["DefaultAccountState<br/>= Frozen<br/>— every new token account<br/>is born locked"]
            PD["PermanentDelegate<br/>= admin (Squads at P1)<br/>— issuer final control,<br/>disclosed, break-glass only"]
            MP["MetadataPointer<br/>→ SELF<br/>— metadata lives in the mint"]
            TM["TokenMetadata (variable)<br/>name · symbol · uri ·<br/>label pairs on-chain:<br/>NOT AN OFFER · backing · nav_precision"]
        end
        BASE --> DAS --> PD --> MP --> TM
    end

    subgraph FORBIDDEN["Deliberately absent (verify.ts asserts)"]
        F1["TransferHook — breaks composability, replaced by Token ACL"]
        F2["TransferFee — the fee is a NAV-side accrual, not a transfer tax"]
        F3["InterestBearing — yield is real sleeve value, not an on-mint index"]
        F4["ConfidentialTransfer — sealed-bid privacy lives at the auction layer"]
        F5["NonTransferable — trading the asset IS the product"]
    end
```

Why frozen-by-default: compliance runs **once at account activation**, not on
every transfer. After thaw, transfers cost **zero extra compute** — routes,
wallets, and future DeFi integrations pay nothing per hop. That is the entire
reason sRFC37 (Token ACL) was chosen over transfer hooks.

## 2 · How the mint is created (`scripts/init-mint.ts`)

The creation sequence is forced by Token-2022's TLV mechanics (each rule
below was proven empirically — see `runbook.md` §mechanics):

```mermaid
sequenceDiagram
    participant A as Admin authority
    participant S as System Program
    participant T as Token-2022 Program
    participant M as NPCS Mint Account

    rect rgb(240, 248, 255)
    note over A,M: tx1 — the exact-fit transaction
    A->>S: createAccount(275 bytes, rent-exact)
    S->>M: account born (zeroed)
    A->>T: InitializeDefaultAccountState(Frozen)
    A->>T: InitializePermanentDelegate(admin)
    A->>T: InitializeMetadataPointer(→ mint itself)
    A->>T: InitializeMint2 — requires length ==<br/>size of extensions so far (exact fit)
    end

    rect rgb(245, 255, 245)
    note over A,M: tx2 — metadata (realloc needs its own tx)
    A->>S: transfer rent delta (+106,880 micro-SOL)
    A->>T: Initialize(name, symbol, uri)<br/>— TLV grows 275→428 inside the mint
    end

    rect rgb(255, 250, 240)
    note over A,M: tx3…n — one tx per label pair
    loop each pair (label, asset_class, nav_precision, backing)
        A->>S: transfer rent delta
        A->>T: UpdateField(key, value) — TLV grows to 581
    end
    end
```

**The three mechanics that shaped this sequence** (all verified against the
spl-token-2022 source and on-chain):

1. `InitializeMint2` rejects any account with trailing space — the account
   must be *exactly* as large as the extensions initialized so far.
2. Metadata ops grow the account internally but never fund it — a rent
   `SystemProgram.transfer` must precede each growth.
3. An account cannot be realloc'd in the transaction that created it
   ("memory allocation failed, out of memory") — metadata lives in later txs.

## 3 · Movement topology — every path a NPCS unit can take

The frozen base layer means all movement is mediated. Eight paths exist;
anything else is impossible at the base layer:

```mermaid
flowchart TB
    THAWED["thawed token account<br/>(owner passed credit-gate)"]

    P1["1 · PRIMARY MINT<br/>Squads mints 1:1 vs confirmed<br/>USDC deposit (invariant I1)"]
    P2["2 · P2P TRANSFER<br/>plain transfer_checked between<br/>thawed accounts — zero extra CU"]
    P3["3 · AUCTION ESCROW<br/>commit: escrow at session open<br/>settle: DvP to gate-checked buyer<br/>refund: permissionless"]
    P4["4 · FACILITY PLEDGE<br/>collateral PDA under<br/>MARK_PROTECTIVE maintenance"]
    P5["5 · CLAIM WRAP<br/>NPCS never moves — a queue-claim<br/>is minted against an attested<br/>redemption request instead"]
    P6["6 · TRANCHE DEPOSIT<br/>pool collateral PDA;<br/>subordination invariant pre-check"]
    P7["7 · SEIZURE<br/>permanent delegate — break-glass only<br/>(sanctions / court / lost keys)<br/>evented with reason hash"]
    P8["8 · REDEMPTION BURN<br/>burn at published NAV;<br/>sleeve pays out; I1 holds"]

    THAWED --> P1 --> THAWED
    THAWED --> P2
    THAWED --> P3
    THAWED --> P4
    THAWED --> P5
    THAWED --> P6
    THAWED --> P7
    THAWED --> P8

    NEG["Impossible by construction:<br/>• move to a frozen account<br/>• reach an AMM (none exists for the raw asset)<br/>• leave the allowlisted universe<br/>• enter DeFi routes (foreign holders' tokens arrive frozen)"]
```

## 4 · Where it goes — the ACL target state

Today the freeze authority is the bootstrap admin wallet. P1 moves it into
*code*:

```mermaid
flowchart LR
    subgraph NOW["NOW (bootstrap)"]
        W["admin wallet<br/>= freeze authority"]
        W -->|"freeze/thaw by<br/>human decision"| FA["frozen accounts"]
    end

    subgraph TARGET["P1 TARGET (sRFC37)"]
        SQ["Squads 2-of-2"] -->|"delegate freeze authority"| MC["Token ACL<br/>MintConfig PDA"]
        MC -->|"names as gate"| CG["credit-gate program"]
        USER["new holder"] -->|"permissionless thaw"| MC
        CG -->|"reads SAS attestations:<br/>kyc · accredited · jurisdiction ·<br/>investor_id · expiry"| OK["approve / reject<br/>typed reason"]
    end
```

**What exists today (code, #122/#131/#133):** the right-hand box is built.
`credit-gate` reads real SAS attestations (`eligibility::evaluate`): SAS
program owner, canonical attestation PDA, the market's credential and
schemas, an attester from `MarketConfig::attesters`, and expiry against the
settlement horizon. `validate` adds the CG-P4 holder governor, and
`can_thaw_permissionless` / `can_freeze_permissionless` are the Token ACL
gate hooks. `settle_check` re-runs the same decision inside every
payment-rails fill. `ops/gate/wire.ts` creates the Token ACL MintConfig and
delegates the freeze authority to it. The schemas, credential and attester
are live on devnet (`ops/gate/deployment-devnet.json`).

**What is not done yet:** wiring the devnet NPCS mint to the deployed gate
(#170/#171, see DEPLOYMENTS.md). Until that is recorded, the left-hand box
is still what runs on devnet.

After wiring, the §2.1 provenance check becomes REQUIRED: the freeze-authority
account's owner must be the Token ACL program — the only structural
discriminator between a regulated permissioned fund and a honeypot (since
March 2026 they share the same on-chain shape; see doc 09 §2.1).
