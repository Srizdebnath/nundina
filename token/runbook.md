# NPCS operations runbook

The pilot asset: a real, USDC 1:1-backed Token-2022 sleeve. Spec: doc 09.
Parameters: `params.toml`. Status: **mint live on devnet, verify ALL CHECKS PASSED.**

## Keys (gitignored — `token/keys/`)

| File | Role | Notes |
|---|---|---|
| `mint.json` | mint account keypair | Only needed at init; may be discarded after (authorities are what matter). **Back up before `cargo clean`** if you ever want to re-init at this address. |
| `freeze-authority.json` | admin authority | Holds mint authority + freeze authority + permanent delegate + metadata update authority until Squads 2-of-2 takes over at P1 (08 §8). **Back this up — it is irreplaceable.** |
| `deployment-<cluster>.json` | audit record | Mint address, extensions, sizes, timestamp. |

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

## ACL wiring (next step, P1)

Freeze authority currently = wallet (bootstrap). At P1:
1. Implement `credit-gate::validate` (CG-P1, the sRFC37 gate interface).
2. Delegate freeze authority to the Token ACL `MintConfig` PDA naming
   `credit-gate` as gate program.
3. Promote verify.ts's §2.1 provenance check from informational to REQUIRED:
   freeze-authority account owner == Token ACL program (`TACLkU6…`).
