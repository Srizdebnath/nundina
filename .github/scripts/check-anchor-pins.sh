#!/usr/bin/env bash
# One anchor version, everywhere (#46, doc 05 D16). The version is taken from
# the root Anchor.toml [toolchain]; every other place that states it must
# match exactly:
#   - every Anchor.toml `anchor_version`
#   - every Cargo.toml `anchor-lang` / `anchor-spl` requirement (must be `=X`)
#   - every Cargo.lock `anchor-lang` / `anchor-spl` package version
#   - every `ANCHOR_VERSION:` in the CI workflows
# and that the host Rust toolchain is pinned the same way (#98):
#   - rust-toolchain.toml `channel` is an exact x.y.z
#   - every workflow `toolchain:` equals it (or is a dated nightly)
#   - no `uses: …@stable` / `@nightly` moving refs
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

# Table form (#98): `[dependencies.anchor-lang]` / `[workspace.dependencies.
# anchor-spl]` + `version = "…"` on a following line. The inline grep above
# never sees these, so a pin written this way used to pass vacuously.
while IFS= read -r f; do
  while IFS=$'\t' read -r sect req; do
    [ "$req" = "=$want" ] || bad "$f" "[$sect] version is '$req', want '=$want' (exact pin)"
  done < <(awk '
    /^\[/ { if (sect != "") print sect "\t" ver; sect = ""; ver = "" }
    /^\[([a-z0-9_.-]+\.)?dependencies\.anchor-(lang|spl)\]/ { sect = substr($0, 2, length($0) - 2) }
    sect != "" && /^version *=/ { v = $0; sub(/^version *= *"/, "", v); sub(/".*/, "", v); ver = v }
    END { if (sect != "") print sect "\t" ver }
  ' "$f")
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

# Host Rust toolchain (#98): rust-toolchain.toml pins an exact version, the
# workflows install exactly that (or a dated nightly), and no action is
# referenced by a moving `@stable` / `@nightly` ref.
rust=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml 2>/dev/null)
if ! [[ "$rust" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  bad rust-toolchain.toml "channel is '$rust', want an exact x.y.z version"
else
  echo "rust toolchain of record: $rust"
  while IFS= read -r hit; do
    f=${hit%%:*}; v=$(sed -n 's/.*toolchain: *\([^ #]*\).*/\1/p' <<<"$hit")
    case "$v" in
      "$rust") ;;
      nightly-20[0-9][0-9]-[01][0-9]-[0-3][0-9]) ;;
      '${{'*) ;;  # resolved from a job env var, checked on the next line
      *) bad "$f" "toolchain: '$v' — want '$rust' (rust-toolchain.toml) or a dated nightly-YYYY-MM-DD" ;;
    esac
  done < <(grep -HE '^ +toolchain:' .github/workflows/*.yml || true)
  while IFS= read -r hit; do
    f=${hit%%:*}; v=$(sed -n 's/.*NIGHTLY: *\([^ #]*\).*/\1/p' <<<"$hit")
    [[ "$v" =~ ^nightly-20[0-9]{2}-[01][0-9]-[0-3][0-9]$ ]] || bad "$f" "NIGHTLY is '$v', want a dated nightly-YYYY-MM-DD"
  done < <(grep -HE '^ +NIGHTLY:' .github/workflows/*.yml || true)
fi
while IFS= read -r hit; do
  bad "${hit%%:*}" "moving toolchain ref: ${hit#*:}"
done < <(grep -HE 'uses: *[^ ]*@(stable|nightly|beta)\b' .github/workflows/*.yml || true)

[ "$fail" = 0 ] && echo "rust toolchain pins agree on ${rust:-?}"
exit "$fail"
