# ops/gate — credit-gate onboarding (#122)

Two tools for putting a mint behind `credit-gate` (SRS 08 §4.1, §5):

| Tool | Commands | Signs with |
|---|---|---|
| `sas.ts` | `setup` (credential `nundina` + the three schemas, idempotent) · `attest <wallet>` (registrar step: identity + accreditation) · `verify` (keyless) | credential authority, attester (`KEYS_DIR`, default `~/.config/nundina/sas-<cluster>/`) |
| `wire.ts` | `root` (GateRoot) · `market` (config + policy + block-list + Token ACL extra metas) · `acl` (mint freeze authority → Token ACL `MintConfig`, gate = credit-gate, permissionless thaw + freeze on) · `validate <wallet>` · `thaw <token-account>` · `verify` (keyless) | `Bprm…` (root), governance (market), the mint's freeze authority (acl); anyone for validate/thaw |

Devnet state: the SAS credential, attester and schemas are live
([`deployment-devnet.json`](deployment-devnet.json)). `root`, `market` and
`acl` follow: credit-gate v0.2.0 is deployed at `5ycY…X6XX` with its root
seeded (governance `EPCf…BavSK`, #170); `market` + `acl` on the NPCS mint
are #171. The exact sequence is in
[`token/runbook.md`](../../token/runbook.md) § ACL wiring.

## How a thaw is decided

`validate` (anyone pays) reads the owner's two SAS attestations and admits
the wallet: block-list → attestations (SAS-owned, canonical PDA, this
credential and schema, signer in `MarketConfig.attesters`) → expiry vs
settlement horizon → jurisdiction / KYC / accreditation policy → holder cap
counted per `investor_id`. Approval writes the holder link and opens the lot
ledger; a rejection is evented with its reason and changes nothing.

`thaw` asks Token ACL to thaw; Token ACL CPIs the gate's
`can_thaw_permissionless`, which re-checks everything that can change
(block-list, attestations still open and in date, policy, same investor).
`can_freeze_permissionless` is the mirror: anyone may freeze a holder the
gate no longer admits, so a revoked attestation is enforceable.

## Tests

```bash
npm ci
npm run typecheck
npm test     # test-localnet.sh: needs `anchor build` at the repo root
```

`test-localnet.sh` runs this checkout's `credit-gate` at its real id with the
real Token ACL and SAS cloned from devnet. It wires an NPCS-shaped mint
(DefaultAccountState = Frozen) end to end. A stranger's permissionless thaw
succeeds for an attested investor and fails for an unattested wallet, and
the issuer's old freeze key can no longer freeze. The program-level suite
(every rejection reason, governor, revocation, the hook refusing calls from
outside Token ACL) is `programs/credit-gate/tests-svm/tests/sas_token_acl.rs`.
