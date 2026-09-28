# Official deployments

Registry of official NUNDINA program deployments (G14: published from day one).
Every row is verifiable independently: program ID on-cluster, dump-hash against
the recorded build hash, upgrade authority against the authority map (08 §8).

| Program | Cluster | Program ID | Slot | Deploy signature | Build SHA-256 | Upgrade authority |
|---|---|---|---|---|---|---|
| credit-gate v0.1.0 (P0 skeleton) | devnet | `7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c` | 503888002 | `2rDLR9z1NDskQsBXMf1jejEKVrCZ5ch7TJRxNTbfL79mZsms5rzigAi6JX4m7PKFLPVYryREVw7Gvz311xKv32dR` | `6e8c50879a0151c313f33c31b79d0ef4f9a945200a08cbbd4985efd75eef0e21` | `BprmmqrzBADdQ2iKARtqcDndWV5k22aUGrkMKeySjKgp` (single key — P0 bootstrap; Squads 2-of-2 takes over at P1 per 08 §8) |

## NPCS asset (Token-2022 mint)

| Asset | Cluster | Mint | Details |
|---|---|---|---|
| NPCS — NUNDINA Pilot Credit Sleeve | devnet | `Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i` | 6dp · DefaultAccountState=Frozen · MetadataPointer(self, TLV metadata in mint, 581B) · PermanentDelegate · all authorities = `2xhjfAdrTrWHUiJrTFhGBa22U2zEYriEEr88Kc6sRXMh` (bootstrap, Squads 2-of-2 at P1) · `verify.ts`: ALL CHECKS PASSED |

## Verification procedure

```bash
solana program show 7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c -u devnet
solana program dump 7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c /tmp/dump.so -u devnet
sha256sum /tmp/dump.so   # must equal the Build SHA-256 above

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
  Backup policy, recovery and the Squads 2-of-2 hand-over for every key in
  this table: [`payment-rails/README.md` § Keys](payment-rails/README.md#keys).
- Verifiable-build reproduction (deterministic SBF flags) lands with the P1
  CI pipeline; until then hashes are recorded from this machine's build.
- **Security (#26):** the credit-gate v0.1.0 build above lets *any* signer
  run `initialize_config` for a mint and become its permanent gate-config
  authority. The fix, which gates it on the program's upgrade authority, is
  on `main` but is **not deployed** until the upgrade authority redeploys.
  Nothing consumes this skeleton yet, but configs created on devnet before
  that upgrade aren't trustworthy.
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
  2. update this row's Slot, signature and Build SHA-256, then re-run
     `verify.sh` (it must now print `local anchor build == live build`);
  3. discard any `MarketConfig` created against the old build (none are
     consumed today), and follow the #50 hand-over note below before the
     bootstrap key is retired.
- **Gate authority hand-over (#50):** a `MarketConfig`'s
  `gate_config_authority` starts as the upgrade authority that initialized
  it, which on devnet today is the single bootstrap key above, not a Squads
  vault. Moving the *program* upgrade authority to Squads does **not** move
  any existing config's authority. Each config moves with its own two-step
  hand-over: the current authority calls `propose_gate_authority(<squads
  vault>)`, then the vault calls `accept_gate_authority`. Do this for every
  config when the bootstrap key is retired, and before the key is
  destroyed. A config whose authority key is lost cannot be governed again.
