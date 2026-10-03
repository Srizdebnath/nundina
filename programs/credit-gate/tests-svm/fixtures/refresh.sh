#!/usr/bin/env bash
# Re-dump the third-party programs the credit-gate suite runs (README.md).
set -euo pipefail
cd "$(dirname "$0")"
solana program dump -u devnet 22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG sas.so
solana program dump -u devnet TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP token_acl.so
sha256sum sas.so token_acl.so
