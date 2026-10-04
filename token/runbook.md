# NPCS operations runbook

The pilot asset: a real, USDC 1:1-backed Token-2022 sleeve. Spec: doc 09.
Parameters: `params.toml`. Status: **mint `6vt1wHVQ…VKL4` live on devnet, gated through Token ACL → credit-gate and Squads-owned (#171).** `CLUSTER=devnet npm run verify` → ALL CHECKS PASSED.

## Keys (gitignored — `token/keys/`)

**Custody status (2026-10-04, #171): Squads 2-of-2 vault** `8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV`
(multisig `CHiMWD3q…B39z7`, 48h timelock; devnet members generated on one
machine, see DEPLOYMENTS.md § Squads) holds mint, permanent-delegate,
metadata-pointer and metadata-update authority, confirmed on-chain by
`verify`. The freeze authority is the Token ACL `MintConfig` PDA
`3AccB1giyy9Jcc9GZXJKKEqLyNHdY1SPcw1jreX9NRDS`, which routes thaws to
credit-gate. **One single key remains:** `freeze-authority.json`
(`7UdtTMgH4v8YSG8mWX2r8uASgJwAkxyo4h8yb5UEEbuh`) is still the MintConfig's
authority (manual Token ACL freeze/thaw), needed to thaw payment-rails'
session vault accounts for the #172 DvP; it moves to the vault right after.
The superseded mint `Dy1VW2…dx2i` and its key `2xhj…` (lost) are a dead
record. Keys of the new mint: `token/keys/` on the minting machine, backed
up in `~/.config/nundina/npcs-devnet/` there (devnet only; mainnet gets a
fresh key ceremony, doc 09 §10).

| File | Role | Notes |
|---|---|---|
| `mint.json` | mint account keypair | Only needed at init; may be discarded after (authorities are what matter). **Back up before `cargo clean`** if you ever want to re-init at this address. |
| `freeze-authority.json` | bootstrap admin | Created the mint; every mint authority it held is now with the Squads vault (#171). Still the Token ACL MintConfig authority until that moves. |
| `deployment-<cluster>.json` | audit record | Private copy; the public record is `token/deployment-<cluster>.json` (committed, read first by `verify`). |

### Backup and verification

- Keep **at least two offline copies** of `freeze-authority.json` in different
  places (for example an encrypted USB drive and a password manager's secure
  file store).
- After every backup or restore, check the copy:
  `solana-keygen pubkey <copy>` must print
  `2xhjfAdrTrWHUiJrTFhGBa22U2zEYriEEr88Kc6sRXMh`.
- Record **who** holds each copy (not where) in the team's private ops notes.
- Move all four authorities to the Squads 2-of-2 at P1 (08 §8); until then
  this section's status line stays ⚠️.

## Operating procedures

```bash
cd token

# Initialize a fresh mint (keypairs must exist in keys/)
CLUSTER=devnet npm run init-mint

# Verify the manifest + provenance posture against a live mint
CLUSTER=devnet npm run verify
# override the mint under test:
CLUSTER=devnet MINT=<address> npm run verify
```

## On-chain status marker (#121)

The mint's TLV metadata carries a `status` pair saying what is and is not in
force; `verify` requires it to start with `TARGET-STATE`. The devnet mint
`6vt1wHVQ…VKL4` (#171) was created with:

```
TARGET-STATE: USDC 1:1 backing is a process rule, not enforced on-chain. IN FORCE: thaw/freeze through Token ACL to credit-gate; mint, permanent-delegate and metadata authority = Squads 2-of-2 vault
```

The superseded mint `Dy1VW2…dx2i` never got the pair: its update authority
`2xhj…` is lost.

The value must equal `STATUS` in `scripts/init-mint.ts`. Changing it later
is a metadata update signed by the Squads vault (a 2-of-2 proposal, 48h).

## Treasury (sleeve) — weekly checklist (treasury-agent, SRS 08 §6)

Manual by design: there is no key automation. Run it before every NAV
publish, and keep the filled checklist with the week's evidence bundle.

1. **Read the sleeve.** Record the USDC value of every sleeve holding, in
   micro-USDC: the USDC balance of each sleeve account at a recorded slot
   (no yield venue is integrated, #173). Record each account address, the
   slot and the raw values.
2. **Read supply.** `spl-token supply <NPCS mint> -u <cluster>` → `supply_base`.
3. **Check I1.** Deposits confirmed to date (from the deposit log) × 10⁶
   must equal `supply_base` exactly. If not, **stop**: no publish until the
   gap is explained and fixed.
4. **Reconcile movements.** Every deposit and withdrawal since the last
   checklist has a transaction signature, and every mint and burn on the
   NPCS mint matches one of them.
5. **Compute NAV** with `credit-math` (`weekly_publish(treasury_micros,
   supply_base, fee_bps, Δt)`). Record `nav_gross`, `fee` and `nav_published`
   in pico, plus the inputs hash.
6. **Sanity bounds.** NAV moves less than the configured deviation versus
   last week. A larger move needs a second person's sign-off and a note.
7. **Sign off.** Two people initial the checklist: the preparer, and a
   reviewer who re-ran steps 2, 3 and 5.

Until `mark-engine` exists, step 5's output is published only as a
documented off-chain mark. Nothing on-chain consumes it yet.

## Token-2022 mechanics (verified empirically + from spl-token-2022 source)

These cost us several failed transactions to establish. They apply to **any**
mint with extensions + TLV metadata:

1. **Exact-fit invariant at InitializeMint2.** The account length must EQUAL
   `try_calculate_account_len` of the extensions initialized *so far*
   (processor.rs L126). No trailing slack, or IM2 fails `InvalidAccountData`.
   → create the account pods-only (275B for DAS+PD+MP).
2. **Metadata ops realloc internally.** `Initialize`/`UpdateField` grow the
   account via spl-type-length-value but do NOT fund it → precede each with a
   `SystemProgram.transfer` of the rent delta (official helpers do exactly
   this: `getAdditionalRentForNewMetadata`).
3. **Realloc must happen in its own transaction.** Growing an account that was
   *created in the same tx* panics: "memory allocation failed, out of memory".
   → tx1: create+pods; tx2: rent-transfer + metadata Initialize; tx3…n: one
   tx per UpdateField.
4. **Self-pointer is mandatory.** The metadata processor rejects
   `metadata != mint`: "Metadata for a mint must be initialized in the mint
   itself." MetadataPointer points at the mint.
5. **UpdateField rewrites the whole metadata struct.** Size targets must
   accumulate pairs, not treat each pair as the only one.
6. **Never hand-roll the UpdateField Key-variant encoding** — use the official
   `createUpdateFieldInstruction` builder (converted web3.js → kit).
7. codama `getTokenSize()` under-computes if given a `TokenMetadata` arg —
   size pods via getTokenSize, add metadata TLV manually (`4 + pack(meta).length`).

## ACL wiring: thaws through credit-gate (#122)

Built and tested; not yet run on the devnet mint. Every step is a command
in [`ops/gate`](../ops/gate/README.md), and the whole sequence passes on a
validator with the real Token ACL and SAS programs (`npm test` there).

1. **Gate deploy** — ✅ done 2026-10-04 (#170): v0.2.0 at
   `5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX`, slot 507271972, row in
   `DEPLOYMENTS.md` (the old `7yRE…Qa9c` program is superseded; its
   `Bprm…` key is lost).
2. **Gate root** — ✅ done in the same session:
   `npm run wire -- root --upgrade-authority <turbine-devnet.json> --root EPCf8gbRaQNqyetsnDxrctgaJjwSzSbxDUSyNBxBavSK`
   (tx `4ZBcLVnc…RYDg1Mc`).
3. **Market** — ✅ 2026-10-04 (#171), governance key `EPCf…BavSK`:
   `npm run wire -- market --authority <governance.json> --mint 6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4 --jurisdictions US --min-kyc 1`
   (txs `2hATiD6o…91Mxwn`, `3t1SFSCN…d9xV`; policy US / KYC ≥ 1 / accreditation
   required, holder cap 8, seasoning 7 days).
4. **Freeze authority → Token ACL** — ✅ (bootstrap admin `7Udt…`):
   `npm run wire -- acl --freeze-authority <freeze-authority.json> --mint 6vt1wHVQ…VKL4`
   (tx `4RLpWUYC…s41TG5`). Done **before** the Squads `move-mint`, which
   leaves a Token ACL freeze authority in place.
5. **Check** — ✅ `npm run wire -- verify --mint 6vt1wHVQ…VKL4` → OK, and
   `CLUSTER=devnet npm run verify` here → ALL CHECKS PASSED (§2.1 REQUIRED).
6. **Each investor:** `npm run sas -- attest <wallet> …` (registrar step),
   `npm run wire -- validate <wallet> --mint …`, then anyone runs
   `npm run wire -- thaw <token-account> --mint …`.
