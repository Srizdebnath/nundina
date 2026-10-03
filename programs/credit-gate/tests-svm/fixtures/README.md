# Third-party program fixtures (#122)

The credit-gate LiteSVM suite runs the gate against the **real** Solana
Attestation Service and Token ACL programs, dumped byte-for-byte from devnet.
Nothing here is built from source in this repo.

| File | Program | Devnet program id | SHA-256 | Dumped |
|---|---|---|---|---|
| `sas.so` | Solana Attestation Service | `22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG` | `afacc7215d6ab6759bcf5edb958a1ad1d9de7559d53ac807c6aa4775a1a5a357` | 2026-10-03 (last deployed slot 385530432) |
| `token_acl.so` | Token ACL (sRFC37) | `TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP` | `bd467aa222517ac76ea76a1c83fee50d9ce4906fdb8eafd6fe2c2727be22015e` | 2026-10-03 (last deployed slot 502030236) |

Re-verify or refresh (keyless):

```bash
bash refresh.sh          # re-dumps both from devnet and prints the SHA-256s
sha256sum -c SHA256SUMS  # what the tests run is what is recorded
```

A refresh that changes a hash is a dependency upgrade: re-run the suite and
update this table in the same change.
