# NPCS — NUNDINA Pilot Credit Sleeve

> **The pilot asset.** A real, USDC 1:1-backed Token-2022 token that NUNDINA issues and controls end-to-end — the asset every mechanism in the protocol (gate, auction, queue-claim, facility, tranching) runs on. **Live on devnet.**

**Status:** mint created · manifest verified · `PILOT-DEMO-ASSET — NOT AN OFFER`

| Fact | Value |
|---|---|
| Mint (devnet) | `Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i` |
| Standard | Token-2022 (Token Extensions), 6 decimals |
| Backing | USDC 1:1 — supply only ever equals confirmed deposits (invariant I1) |
| Default state | **Frozen** — every token account is born locked; thaws only through `credit-gate` |
| Metadata | TLV **inside the mint** (self-pointer), label pairs on-chain incl. `PILOT-DEMO-ASSET — NOT AN OFFER — SYNTHETIC LABEL` |
| Authorities | single admin key (bootstrap) — Squads 2-of-2 at P1; freeze authority moves to Token ACL `MintConfig` PDA when the gate wires in |
| Size | 275 B (pods) → 581 B (with metadata) |

## Layout

```
token/
├── params.toml          ← single source of truth for every parameter
├── metadata.json        ← off-chain metadata (the on-chain uri field points here)
├── runbook.md           ← ops procedures + verified Token-2022 mechanics
├── ARCHITECTURE.md      ← how the token is built (mermaid diagrams)
├── TOKENOMICS.md        ← the three invariants + the math (mermaid)
├── scripts/
│   ├── init-mint.ts     ← creates the mint with the exact extension manifest
│   └── verify.ts        ← asserts manifest + provenance against a live mint
├── tests/               ← (P1: LiteSVM mint specs)
└── keys/                ← gitignored: mint + authority keypairs, deployment records
```

## Quick start

```bash
cd token
npm install

# verify the live devnet mint (no keys needed)
CLUSTER=devnet npm run verify

# initialize a fresh mint (keypairs must exist in keys/)
CLUSTER=devnet npm run init-mint
```

## What this token is not

Not a protocol token. No distribution, no vesting, no emissions, no governance,
no staking. It is a *pilot fund share*: one base unit per USDC micro deposited,
redeemable at the published NAV. The securities posture is deliberate — see
`../preresearch/architecture/07` (L24: no protocol token) and doc 09 §1.1 for
the ONyc/ACRED replication map.

## Verification

Anyone can independently confirm the live mint's configuration:

```bash
cd token && CLUSTER=devnet npm run verify
```

Asserts: Token-2022 ownership, 6dp, authority identities, frozen-by-default,
self-pointer, on-chain NOT-AN-OFFER label, backing/nav labels, and absence of
every forbidden extension. Exit code 1 on any failure.

Registry entry: `../DEPLOYMENTS.md`.
