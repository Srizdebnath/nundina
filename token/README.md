# NPCS — NUNDINA Pilot Credit Sleeve

> **The pilot asset.** A Token-2022 token that NUNDINA issues and controls end-to-end — the asset every mechanism in the protocol (gate, auction, queue-claim, facility, tranching) is designed to run on. **Mint live on devnet.**
>
> **Honest status (#171, 2026-10-04):** a **new devnet mint** replaced `Dy1VW2…dx2i`, whose only key (`2xhj…`) is lost. It is **gated** (freeze authority = Token ACL `MintConfig` → `credit-gate`; investors thaw only through the gate) and **Squads-owned** (mint, permanent delegate, metadata authority = the 2-of-2 vault). It is **not backed by code**: supply 1.000000 matches 1 USDC deposited by hand into the vault's sleeve account, and nothing on-chain enforces I1. One authority is still a single key: the Token ACL MintConfig authority `7Udt…` (manual freeze/thaw, needed to thaw payment-rails vault accounts for the #172 DvP). [`TOKENOMICS.md`](TOKENOMICS.md) §0 says what is enforced.

**Status:** mint created · manifest + §2.1 provenance verified · gated · Squads-owned · `PILOT-DEMO-ASSET — NOT AN OFFER`

| Fact | Value |
|---|---|
| Mint (devnet) | `6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4` (supersedes `Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i`) |
| Standard | Token-2022 (Token Extensions), 6 decimals |
| Backing | USDC 1:1 **by process** (invariant I1): 1 USDC in the vault sleeve account `B78WTdsY…NKAz` (tx `4hKqSJS3…623nHE`) before the 1.000000 NPCS mint (tx `5QP69Ur2…sUxJ57a`). No program enforces it; there is no mint-on-deposit code |
| Default state | **Frozen**: every token account is born locked. Investors thaw through Token ACL `ThawPermissionless` → `credit-gate` `can_thaw_permissionless` (SAS attestations + `validate`); anyone can freeze a holder the gate no longer admits ([ARCHITECTURE §4](ARCHITECTURE.md)) |
| Metadata | TLV **inside the mint** (self-pointer), label pairs on-chain incl. `PILOT-DEMO-ASSET — NOT AN OFFER — SYNTHETIC LABEL` |
| Authorities | mint, permanent delegate, metadata pointer + update = Squads vault `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV` (2-of-2, 48h); freeze = Token ACL `MintConfig` `3AccB1gi…NRDS`, whose own authority is still the bootstrap key `7UdtTMgH4v8YSG8mWX2r8uASgJwAkxyo4h8yb5UEEbuh` (DEPLOYMENTS.md § Squads row 3) |
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
├── tests/               ← offline drift tests (params.toml vs metadata, credit-math, verify.ts)
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
