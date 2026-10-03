# NPCS operations runbook

The pilot asset: a real, USDC 1:1-backed Token-2022 sleeve. Spec: doc 09.
Parameters: `params.toml`. Status: **mint live on devnet.** Since #121, `verify` fails on the devnet mint until the key holder writes the on-chain status marker below (one command); every other check passes.

## Keys (gitignored — `token/keys/`)

**Custody status (2026-09, #57, #99): ⚠️ single key, no recorded backup.**
`freeze-authority.json` (`2xhjfAdrTrWHUiJrTFhGBa22U2zEYriEEr88Kc6sRXMh`)
is the highest-privilege key in the repo: confirmed on-chain
(`spl-token display <mint> -u devnet`) as mint, freeze, permanent-delegate
and metadata-update authority. No offline backup of it is recorded, and it
is not under Squads. The backup policy below is the plan, not a record that
it was executed. Loss: no minting, no thaw of new holders, no seizure path,
no metadata fix. Theft: unlimited mint, arbitrary freeze and seizure.
Recovery: none on this mint, only a fresh mint with a new key ceremony
(doc 09 §10). Custodian: recorded in the team's private ops notes, never in
this repo (same rule as [`payment-rails/README.md` § Keys](../payment-rails/README.md#keys)).

| File | Role | Notes |
|---|---|---|
| `mint.json` | mint account keypair | Only needed at init; may be discarded after (authorities are what matter). **Back up before `cargo clean`** if you ever want to re-init at this address. |
| `freeze-authority.json` | admin authority | Holds mint authority + freeze authority + permanent delegate + metadata update authority until Squads 2-of-2 takes over at P1 (08 §8). **Back this up — it is irreplaceable.** |
| `deployment-<cluster>.json` | audit record | Mint address, extensions, sizes, timestamp. |

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

The mint's TLV metadata carries `backing = USDC_1to1` and the off-chain
`metadata.json` names the Token ACL → `credit-gate` model. Neither is in
force. Mints made by `init-mint.ts` after #121 also carry a `status` pair
that says so; `verify` requires it. The devnet mint predates #121, so the
holder of `freeze-authority.json` (the metadata update authority) adds it
once:

```bash
spl-token update-metadata Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i status \
  "TARGET-STATE: backing, Token ACL/credit-gate gating and Squads custody are NOT in force; one bootstrap key holds every authority" \
  --authority keys/freeze-authority.json --fee-payer keys/freeze-authority.json -u devnet
CLUSTER=devnet npm run verify   # → ALL CHECKS PASSED
```

The value must equal `STATUS` in `scripts/init-mint.ts`. Replace it (same
command) when the gate, backing or Squads custody actually land.

## Treasury (sleeve) — weekly checklist (treasury-agent, SRS 08 §6)

Manual by design: there is no key automation. Run it before every NAV
publish, and keep the filled checklist with the week's evidence bundle.

1. **Read the sleeve.** Record the USDC value of every sleeve holding, in
   micro-USDC: the Kamino vault shares × the share price read at a recorded
   slot, or, in backup mode, the plain USDC account balance. Record each
   account address, the slot and the raw values.
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

1. **Gate redeploy** (holder of `Bprm…jKgp`, #45): `anchor build`, then
   `solana program deploy … --program-id 7yREa5V3…Qa9c --upgrade-authority <Bprm.json>`;
   record the row in `DEPLOYMENTS.md`.
2. **Gate root** (same key): `npm run wire -- root --upgrade-authority <Bprm.json> --root <governance key>`.
3. **Market** (the root/governance key): `npm run wire -- market --authority <kp> --mint Dy1VW2uC8G3PxDkfmJya1tRrzSTWQ18GkbN3qh3mdx2i`
   (config, policy US / KYC ≥ 1 / accreditation required, block-list, Token ACL extra metas).
4. **Freeze authority → Token ACL** (holder of `2xhjfAdr…6sRXMh`):
   `npm run wire -- acl --freeze-authority <2xhj.json> --mint Dy1VW2…dx2i`.
   From this transaction on, no wallet can freeze or thaw NPCS by hand:
   investors thaw through the gate, anyone can freeze a holder the gate no
   longer admits. Do this **before** moving the remaining NPCS authorities to
   Squads (`DEPLOYMENTS.md` § Squads row 3): the freeze authority goes to
   Token ACL, not to the vault.
5. **Check:** `npm run wire -- verify --mint Dy1VW2…dx2i` and
   `CLUSTER=devnet npm run verify` here: the §2.1 provenance check is
   REQUIRED and passes only when the freeze authority is Token ACL's
   `MintConfig` routing to `credit-gate` with permissionless thaw and freeze on.
6. **Each investor:** `npm run sas -- attest <wallet> …` (registrar step),
   `npm run wire -- validate <wallet> --mint …`, then anyone runs
   `npm run wire -- thaw <token-account> --mint …`.
