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
| payment-rails v0.1.0 (cash layer, R5) | devnet | `GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn` | 504436527 | `3aWLZJ9WegXPJXkTrQHY2hpmTTm8PQc3Ztmd1EENAXxSc3AgHAfZL6yMqspA1QRpkEmi2XeD27c3sHtRwU5ioR3w` | `35ace429088050215fdb682ac9dcabc7834e93d50af73d41cbc7a30f4bcb7f38` | `9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ` (single key, pilot; Squads 2-of-2 per 08 §8 before mainnet) |
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

- The P0 skeleton contains only the CG-P2 config anchor (initialize/update
  config). It carries **no gate entrypoint yet** — nothing consumes it; it
  proves the build–deploy–verify loop and pins the program ID early.
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
- **Squads 2-of-2 migration: dated schedule (#126).** Not executed. Every
  authority below is still a single key; this table is the plan, and a row
  counts as done only when its *Executed* cell holds a devnet signature.
  The highest-blast-radius key moves first. The two vault members (founder
  + ops, SRS 08 §8) are named by the key holders; a date that slips gets a
  new date here, never a blank.

  | # | Authority | Held by today | Moves to | How | Target date | Executed |
  |---|---|---|---|---|---|---|
  | 0 | — | — | Squads 2-of-2 multisig on devnet, 48h timelock on upgrades | Squads app or CLI, members = founder + ops; record the multisig and vault PDA here | 2026-10-08 | ☐ |
  | 1 | Offline backups of every key below until its row is done | — | two offline copies each ([`token/runbook.md`](token/runbook.md) § Backup) | record *that* a backup exists, by custodian, in the private ops notes | 2026-10-08 | ☐ |
  | 2 | NPCS mint, freeze, permanent delegate, metadata update | `2xhjfAdr…6sRXMh` (if stolen: unlimited mint and freeze) | vault PDA (freeze authority moves on to the Token ACL `MintConfig` when the gate wires in) | `spl-token authorize <mint> mint <vault>` and `… freeze <vault>`; the delegate and metadata authorities via Token-2022 `SetAuthority` / `UpdateMetadataAuthority` | 2026-10-09 | ☐ |
  | 3 | `payment-rails` upgrade authority | `9WH1…2uDZ` | vault PDA | `solana program set-upgrade-authority` ([`payment-rails/README.md`](payment-rails/README.md) § Before mainnet) | 2026-10-09 | ☐ |
  | 4 | `credit-gate` upgrade authority + gate root + every config | `Bprm…jKgp` | vault PDA | after the #45 redeploy: `set-upgrade-authority`, then `propose_root_authority` → `accept_root_authority`, then each config per #50 above | 2026-10-10 | ☐ |

  After each row: re-run `verify.sh` / `npm run verify`, put the signature
  and the new authority in the registry rows at the top, and update
  `token/metadata.json` `authority_model`.
