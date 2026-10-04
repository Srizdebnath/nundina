# ops/squads — Squads v4 2-of-2 authority migration (#126)

SRS 08 §8: every administrative authority (program upgrades, NPCS mint,
freeze, permanent delegate and metadata) sits behind a Squads 2-of-2 with a
48h timelock. `migrate.ts` does the migration and proves it; the devnet
result is recorded in [`deployment-devnet.json`](deployment-devnet.json) and
in the root [`DEPLOYMENTS.md`](../../DEPLOYMENTS.md).

| Command | What it does |
|---|---|
| `create` | Creates the multisig: threshold 2, both members with all permissions, `TIMELOCK_SECONDS` (default 172800 = 48h), **no config authority** (members and threshold change only by a 2-of-2 vote). Writes the record. Refuses if a record exists |
| `move-program <id> --authority <kp>` | Loader v3 `SetAuthority` → vault. Refuses unless the signer is the current authority; re-reads ProgramData and fails unless the vault now holds it |
| `move-mint <mint> --authority <kp>` | Token-2022: mint, freeze, permanent delegate, metadata pointer **and** metadata update authority → vault, in one transaction. Refuses if any of them is held by a key other than the signer (no silent partial custody) |
| `prove` | Vault transaction (0-lamport self-transfer), proposal, approval by **both** members, then execute once the timelock allows (re-run to resume) |
| `propose-upgrade <program> --buffer <buf> [--spill <addr>]` | A program upgrade signed by the vault (#172). The buffer must already hold the new build with the vault as its authority (`solana program write-buffer` + `set-buffer-authority`). Extends ProgramData first if the build is larger, using the loader's permissionless `ExtendProgram` paid by `PAYER` (a Squads CPI can grow an account by only 10 KiB). Then the proposal and both approvals, with the buffer's SHA-256 in the record. Execution waits for the timelock |
| `execute-upgrade <program>` | Executes the pending upgrade once the timelock allows (simulated first, so a failure prints the program logs) |
| `verify` | Keyless: re-reads the multisig (2-of-2, members, timelock, no config authority), the vault address and every recorded move from chain |

Env: `CLUSTER` (`devnet` / `localnet`), `RPC_URL`, `PAYER` (fee and rent
payer; members never need SOL), `KEYS_DIR` (member keys; default `./keys`,
gitignored), `TIMELOCK_SECONDS`.

```bash
npm ci
npm run typecheck
npm test            # test-localnet.sh: real Squads v4 cloned from devnet
```

`test-localnet.sh` runs every command against a local validator with the
real Squads v4 program and its `ProgramConfig`, a throwaway upgradeable
program and an NPCS-shaped Token-2022 mint (freeze, permanent delegate,
metadata pointer + metadata), with timelock 0. It also checks that the old
keys lose every power (no upgrade-authority change, no mint), that a move by
a non-holder is refused, and that a vault transaction with **one** approval
cannot execute.

SDK note: `@sqds/multisig` 2.1.4's `rpc.proposalCreate` accepts `rentPayer`
but does not pass it to the instruction, so the member would pay the rent;
`migrate.ts` builds that instruction directly.

## Devnet custody caveat

The devnet members were generated on one machine
(`~/.config/nundina/squads-devnet/`, outside the repo). That demonstrates the
mechanism, not two-person custody. For mainnet each member is a different
person's hardware wallet.
