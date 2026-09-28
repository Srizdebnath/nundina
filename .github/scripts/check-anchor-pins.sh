#!/usr/bin/env bash
# One anchor version, everywhere (#46, doc 05 D16). The version is taken from
# the root Anchor.toml [toolchain]; every other place that states it must
# match exactly:
#   - every Anchor.toml `anchor_version`
#   - every Cargo.toml `anchor-lang` / `anchor-spl` requirement (must be `=X`)
#   - every Cargo.lock `anchor-lang` / `anchor-spl` package version
#   - every `ANCHOR_VERSION:` in the CI workflows
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

want=$(sed -n 's/^anchor_version *= *"\(.*\)"/\1/p' Anchor.toml)
[ -n "$want" ] || { echo "::error::no anchor_version in Anchor.toml [toolchain]"; exit 1; }
echo "anchor version of record: $want"
fail=0
bad() { echo "::error file=$1::$2"; fail=1; }

while IFS= read -r f; do
  got=$(sed -n 's/^anchor_version *= *"\(.*\)"/\1/p' "$f")
  [ "$got" = "$want" ] || bad "$f" "anchor_version is '$got', want '$want'"
done < <(git ls-files '*Anchor.toml')

while IFS= read -r f; do
  while IFS= read -r line; do
    req=$(sed -n 's/.*version *= *"\([^"]*\)".*/\1/p' <<<"$line")
    [ -n "$req" ] || req=$(sed -n 's/^anchor-[a-z]* *= *"\([^"]*\)".*/\1/p' <<<"$line")
    [ "$req" = "=$want" ] || bad "$f" "'${line%%=*}' requirement is '$req', want '=$want' (exact pin)"
  done < <(grep -E '^anchor-(lang|spl) *=' "$f" || true)
done < <(git ls-files '*Cargo.toml')

while IFS= read -r f; do
  while IFS= read -r v; do
    [ "$v" = "$want" ] || bad "$f" "locked anchor-lang/anchor-spl is $v, want $want"
  done < <(awk '/^name = "anchor-(lang|spl)"$/{getline; gsub(/version = |"/,""); print}' "$f")
done < <(git ls-files '*Cargo.lock')

while IFS= read -r hit; do
  f=${hit%%:*}; v=$(sed -n 's/.*ANCHOR_VERSION: *\([^ ]*\).*/\1/p' <<<"$hit")
  [ "$v" = "$want" ] || bad "$f" "ANCHOR_VERSION is '$v', want '$want'"
done < <(grep -H 'ANCHOR_VERSION:' .github/workflows/*.yml || true)

[ "$fail" = 0 ] && echo "all anchor pins agree on $want"
exit "$fail"
