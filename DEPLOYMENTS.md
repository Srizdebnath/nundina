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
| credit-gate v0.1.0 (P0 skeleton) | devnet | `7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c` | 503888002 | `2rDLR9z1NDskQsBXMf1jejEKVrCZ5ch7TJRxNTbfL79mZsms5rzigAi6JX4m7PKFLPVYryREVw7Gvz311xKv32dR` | `6e8c50879a0151c313f33c31b79d0ef4f9a945200a08cbbd4985efd75eef0e21` | `BprmmqrzBADdQ2iKARtqcDndWV5k22aUGrkMKeySjKgp` (single key — P0 bootstrap; Squads 2-of-2 takes over at P1 per 08 §8) |

## NPCS asset (Token-2022 mint)

| Asset | Cluster | Mint | Details |
|---|---|---|---|
| NPCS — NUNDINA Pilot Credit Sleeve | devnet | `Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i` | 6dp · DefaultAccountState=Frozen · MetadataPointer(self, TLV metadata in mint, 581B) · PermanentDelegate · all authorities = `2xhjfAdrTrWHUiJrTFhGBa22U2zEYriEEr88Kc6sRXMh` (bootstrap, Squads 2-of-2 at P1) · `verify.ts`: every manifest check passes; the #121 on-chain `status` marker is not written yet (`token/runbook.md` § On-chain status marker) |

## Verification procedure

```bash
solana program show 7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c -u devnet
solana program dump 7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c /tmp/dump.so -u devnet
sha256sum /tmp/dump.so   # must equal the Build SHA-256 above

# payment-rails (keyless; fails closed on any mismatch):
bash payment-rails/crank/scripts/devnet/verify.sh

# NPCS asset manifest + provenance:
cd token && CLUSTER=devnet npm run verify
```

## Notes

- The live devnet build is the P0 skeleton: only the CG-P2 config anchor.
  `main` now carries CG-P1..P7 (SAS read, governor, block-list, lot ledger,
  `validate`, Token ACL hooks; #122). Going live is the key holders' run in
  [`ops/gate/README.md`](ops/gate/README.md): redeploy (below), `wire.ts root`,
  `wire.ts market`, then the NPCS freeze-authority hand-over `wire.ts acl`.
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
- **Security (#26):** the credit-gate v0.1.0 build above lets *any* signer
  run `initialize_config` for a mint and become its permanent gate-config
  authority. The fix on `main` gates it on the gate root (#93; seeded only
  by the program's upgrade authority) and is **not deployed** until the
  upgrade authority redeploys. Nothing consumes this skeleton yet, but
  configs created on devnet before that upgrade aren't trustworthy.
- **Checking and closing the #45 gap.** Anyone can run
  `bash programs/credit-gate/scripts/verify.sh`. It verifies the row above
  keylessly (source id, genesis, dump SHA-256, authority, slot) and reports
  whether this checkout's `anchor build` equals the live build. Today it
  does not: the live build is the pre-#26 one. The redeploy needs the
  upgrade authority `Bprm…jKgp`:
  1. `anchor build` on `main`, then `solana program deploy
     target/deploy/credit_gate.so --program-id 7yREa5V3…Qa9c
     --upgrade-authority <Bprm….json> -u devnet` (bare id: the program
     keypair is not needed);
  2. call `initialize_root(<root authority>)`, signed by the upgrade
     authority, **in the same session** — until the root exists no market
     can be onboarded, and it must exist before the program is ever
     finalized (a finalized program with no root can never seed one);
  3. update this row's Slot, signature and Build SHA-256, then re-run
     `verify.sh` (it must now print `local anchor build == live build`);
  4. discard any `MarketConfig` created against the old build (none are
     consumed today), and follow the #50 hand-over note below before the
     bootstrap key is retired.
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
  | 4 | `credit-gate` upgrade authority | `Bprm…jKgp` | vault | ☐ **key holder runs**, after the #45 redeploy: `CLUSTER=devnet npx tsx migrate.ts move-program 7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c --authority <Bprm….json>`; then the gate root and each config per #50 above (accept signed by the vault through a Squads proposal) |

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
