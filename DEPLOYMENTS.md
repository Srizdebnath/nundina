# Official deployments

Registry of official NUNDINA program deployments (G14: published from day one).
Every row is verifiable independently: program ID on-cluster, dump-hash against
the recorded build hash, upgrade authority against the authority map (08 §8).

**Two registries, one rule.** This file lists the *current* official
deployment of every program. `payment-rails` keeps its full per-deploy
history, with genesis hashes, in
[`payment-rails/DEPLOYMENTS.md`](payment-rails/DEPLOYMENTS.md), written by its
`deploy.sh`. When they disagree, that file wins, and this row must be updated
to match.

| Program | Cluster | Program ID | Slot | Deploy signature | Build SHA-256 | Upgrade authority |
|---|---|---|---|---|---|---|
| payment-rails v0.1.0 (cash layer, R5) | devnet | `GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn` | 504436527 | `3aWLZJ9WegXPJXkTrQHY2hpmTTm8PQc3Ztmd1EENAXxSc3AgHAfZL6yMqspA1QRpkEmi2XeD27c3sHtRwU5ioR3w` | `35ace429088050215fdb682ac9dcabc7834e93d50af73d41cbc7a30f4bcb7f38` | `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV`: Squads v4 vault, 2-of-2 multisig `CHiMWD3q1eHZ8sfUCPzQPp9UWDQ7x2K75notiq1B39z7`, 48h timelock (#126; was `9WH1…2uDZ`) |
| credit-gate v0.2.0 (CG-P1..P7: SAS read, validate, governor, block-list, lot ledger, Token ACL hooks, settle_check; #170) | devnet | `5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX` | 507268500 | `5CgLegBtQ8PtNto9JweTCXP8AiT14eqaHiJzVvh55iMkzGioCXq11pnkfAfgFBz75spncReYmh4bBmXdPPe8BPL9` | `4866ec513fbb21da9a25e01252470eab6f8746bfaeda7c402237001622f3f457` | `9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ` (deployer, single key — Squads hand-over pending, Squads row 4 below) |

**Superseded credit-gate deployment (#146, #170).** The P0 skeleton at
`7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c` (slot 503888002, build
`6e8c5087…0e21`, upgrade authority `Bprm…jKgp`) can never be upgraded: no
one on the team holds `Bprm…` or its seed phrase. Instead of upgrading it,
the current source was deployed on 2026-10-04 at a **new program id** under
new keys (row above). The old id is a dead record: nothing in this repo
references it, and payment-rails `CREDIT_GATE_ID` now names the new id.
Configs created against the old program are not trusted (#26).

## NPCS asset (Token-2022 mint)

| Asset | Cluster | Mint | Details |
|---|---|---|---|
| NPCS — NUNDINA Pilot Credit Sleeve | devnet | `Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i` | 6dp · DefaultAccountState=Frozen · MetadataPointer(self, TLV metadata in mint, 581B) · PermanentDelegate · all authorities = `2xhjfAdrTrWHUiJrTFhGBa22U2zEYriEEr88Kc6sRXMh` (bootstrap, Squads 2-of-2 at P1) · `verify.ts`: every manifest check passes; the #121 on-chain `status` marker is not written yet (`token/runbook.md` § On-chain status marker) |

## Verification procedure

```bash
solana program show 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX -u devnet
solana program dump 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX /tmp/dump.so -u devnet
sha256sum /tmp/dump.so   # must equal the Build SHA-256 above

# payment-rails (keyless; fails closed on any mismatch):
bash payment-rails/crank/scripts/devnet/verify.sh

# NPCS asset manifest + provenance:
cd token && CLUSTER=devnet npm run verify
```

## Notes

- **The live devnet build is the current source (#170).** Deployed
  2026-10-04 by `9WH1…2uDZ` with `anchor build` on this branch; the dump
  SHA-256 equals the local build (`4866ec51…f457`). The gate root was
  seeded in the same session: `initialize_root(EPCf8gbRaQNqyetsnDxrctgaJjwSzSbxDUSyNBxBavSK)`
  (devnet gate governance key, `~/.config/nundina/credit-gate-devnet/`),
  tx `4ZBcLVncsJ7cyf9xAZXeJuUj5661kw4BGdimymZUQPuVs98iV4DfQiBJFbrsvNj2PGCk5BGpjBZftkWUJYDRg1Mc`.
  Next: `wire.ts market` + `acl` on the NPCS mint (#171/#142), then the
  devnet payment-rails upgrade (Squads proposal, 48h) so its fills CPI
  this gate's `settle_check`.
- The program keypair (`target/deploy/credit_gate-keypair.json`, gitignored)
  is the only way to deploy at this fixed ID on a new cluster. Back it up
  before any `cargo clean`. Upgrades of the live program need only the
  upgrade authority, which makes that key the one whose loss is permanent.
  Every key in this file (credit-gate, NPCS admin, payment-rails, plus the
  rails' fee and reserve accounts) is listed, by public key, in
  [`payment-rails/README.md` § Keys](payment-rails/README.md#keys), with what
  losing or leaking it costs. As of 2026-09 **none has a recorded offline
  backup and none is under Squads** (#57); that section's backup policy and
  hand-over commands are the plan, not a record.
- Verifiable-build reproduction (deterministic SBF flags) lands with the P1
  CI pipeline; until then hashes are recorded from this machine's build.
- **Security (#26):** the superseded v0.1.0 build at `7yRE…Qa9c` lets
  *any* signer run `initialize_config` for a mint and become its permanent
  gate-config authority. The deployed v0.2.0 gates it on the gate root
  (#93), seeded at deploy (above). Configs on the old program are ignored.
- **Checking the deployment (#45).** Anyone can run
  `bash programs/credit-gate/scripts/verify.sh`. It verifies the row above
  keylessly (source id, genesis, dump SHA-256, authority, slot) and reports
  whether this checkout's `anchor build` equals the live build. A future
  upgrade: `anchor build`, `solana program deploy target/deploy/credit_gate.so
  --program-id 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX --upgrade-authority
  <current authority>`, then update this row and re-run `verify.sh`. Once
  the authority is the Squads vault, that is a Squads vault transaction
  with the 48h timelock.
- **Gate root (#93):** `initialize_config` is authorized by the singleton
  `GateRoot` PDA (`["gate_root"]`), not by `ProgramData`, so per-mint
  onboarding (CG-P2, G14) keeps working after doc 06 §11 U1 stages the
  program to immutable. The root is seeded once by the upgrade authority
  and can name a Squads vault directly; it moves only by
  `propose_root_authority` + `accept_root_authority`. Moving the program
  upgrade authority does **not** move the root.
- **What the upgrade authority can still do (#93).** Whoever can replace
  the program binary can rewrite any `MarketConfig` (or the root) with a
  build that ignores these checks: no on-chain guard survives an upgrade
  that removes it. Until the program is finalized, the only control is the
  upgrade authority itself — SRS 08 §8's Squads 2-of-2 **with its 48h
  timelock** — which is not in place today (single bootstrap key, #57).
- **Gate authority hand-over (#50):** a `MarketConfig`'s
  `gate_config_authority` starts as the gate root authority that
  initialized it, which on devnet will be the single bootstrap key above,
  not a Squads vault. Moving the *program* upgrade authority or the root to
  Squads does **not** move any existing config's authority. Each config
  moves with its own two-step hand-over: the current authority calls
  `propose_gate_authority(<squads vault>)`, then the vault calls
  `accept_gate_authority` (which refunds the proposal's rent to the vault,
  #96). Do this for every config when the bootstrap key is retired, and
  before the key is destroyed. A config whose authority key is lost cannot
  be governed again.
- **Squads 2-of-2 migration (#126): executed on devnet for every key this
  machine holds.** Tool: [`ops/squads/`](ops/squads/README.md) (`migrate.ts`,
  end-to-end tested against the real Squads v4 program on a local validator).
  Full record with every signature: [`ops/squads/deployment-devnet.json`](ops/squads/deployment-devnet.json);
  `npx tsx migrate.ts verify` re-checks it keylessly.

  | # | Authority | Was | Now | Executed (devnet) |
  |---|---|---|---|---|
  | 0 | Squads v4 multisig, threshold **2-of-2**, timelock **48h**, no config authority (only a 2-of-2 vote changes members) | — | `CHiMWD3q1eHZ8sfUCPzQPp9UWDQ7x2K75notiq1B39z7` · vault `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV` | ✅ 2026-10-03 · `5dyaTmrZ…b1aWH` |
  | 1 | `payment-rails` upgrade authority | `9WH1…2uDZ` | vault | ✅ 2026-10-03 · `519n9Dw2…VstjP9`; `verify.sh` passes against the new row in `payment-rails/DEPLOYMENTS.md` |
  | 2 | Vault answers to 2-of-2 | — | proposal #1 approved by both members | ✅ approvals 2026-10-03; execute opens 2026-10-05 10:00 UTC (timelock) — `npx tsx migrate.ts prove` |
  | 3 | NPCS mint, freeze, permanent delegate, metadata pointer, metadata update | `2xhjfAdr…6sRXMh` | vault (freeze moves on to the Token ACL `MintConfig` when the gate wires in) | ☐ **key holder runs** `CLUSTER=devnet npx tsx migrate.ts move-mint Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i --authority <2xhj….json>` (the same command passed on an NPCS-shaped mint in `test-localnet.sh`) |
  | 4 | `credit-gate` upgrade authority (new id `5ycY…X6XX`, #170; the old `7yRE…Qa9c` / `Bprm…` program is superseded) | `9WH1…2uDZ` | vault | ☐ **pending** `CLUSTER=devnet npx tsx migrate.ts move-program 5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX --authority ~/.config/solana/turbine-devnet.json`; then the gate root and each config per #50 above (accept signed by the vault through a Squads proposal) |

  **Custody caveat (devnet only):** both member keys were generated on one
  machine (`~/.config/nundina/squads-devnet/`, mode 600, outside the repo).
  That proves the mechanism (2 approvals required, 48h timelock, old key
  powerless) but is not 2-of-2 *custody*. For mainnet the members are two
  people's own hardware wallets; `create` takes no member key from the repo.
  The `RailsConfig` roles (`authority`, `operator`, `registrar`) still sit
  on `9WH1…2uDZ`: they are program-level roles, not the upgrade authority.

## SAS attester (devnet, #122)

| Item | Address |
|---|---|
| SAS program | `22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG` |
| Credential `nundina` | `7H1P4RLB9VhsrSHwqtxJJsMC5wMtF9T7HUN6EUovhfxV` (authority `ErrdHFMVEDdY8idFNKrr4TBsfBgsedxFt89otFm2xCc7`) |
| Authorized signer (attester) | `2L8VPRJme9bEtqoBRTG3kR9JkfbT3GF1DBf48vp2ksQC` |
| `nundina.identity.v1` `[U128, String, U8]` | `6ooYMTCt89VLStUxPPWdTq19Ee9E9TaQCnJPxt3abRUL` |
| `nundina.accreditation.v1` `[Bool, U8]` | `FVxSDQnri8zSREZTXjdgdXc2D8KPyNSFWFsUjG3BuWaU` |
| `nundina.redemption_request.v1` `[VecU8, U64, I64, U32]` | `tdbagMb6dFGBTF83onfstpoeJqBMvHvsgV6m1VQniHs` |

Signatures: `ops/gate/deployment-devnet.json`. Keyless check:
`cd ops/gate && npm ci && npm run sas -- verify`. A team-wallet attestation
pair was issued to the operator wallet `9WH1…2uDZ` (registrar step, pilot
policy: team wallets only) and decodes exactly as the gate's CG-P6 parser
expects.
