# Shared by deploy.sh, verify.sh and programs/credit-gate/scripts/verify.sh.
# Source it; it defines functions only and runs nothing.
#
# An RPC URL can carry a provider key in a query parameter
# (Helius: `/?api-key=…`) or as a path segment (QuickNode: `/<token>/`,
# Alchemy: `/v2/<key>`). The solana CLI echoes the full URL in its errors
# and may normalise it first (add a `/` before `?`, percent-encode), so a
# literal `$RPC` → `$RPC_SHOWN` swap alone can miss it (#97). Everything a
# script prints passes through `redact`, which also scrubs by pattern.

# Pattern scrub for any text: key-like query parameters, and any URL path
# segment of 16+ token characters (hostnames never match: they hold dots).
redact_urls() {
  sed -E \
    -e 's/([?&][A-Za-z_-]*(key|token|secret)[A-Za-z_-]*=)[^&[:space:]")]*/\1<redacted>/Ig' \
    -e ':a' \
    -e 's#(https?://[^/[:space:]"'"'"')]+/([^[:space:]"'"'"')]*/)?)[A-Za-z0-9_-]{16,}([/?#[:space:]"'"'"')]|$)#\1<redacted>\3#' \
    -e 'ta'
}

# The endpoint as it may be shown: no query string, path tokens redacted.
rpc_shown() { printf '%s\n' "${1%%\?*}" | redact_urls; }

# Filter: literal swap of the raw endpoint first, then the pattern scrub.
# Needs RPC and RPC_SHOWN set.
redact() {
  local s
  while IFS= read -r s || [ -n "$s" ]; do
    printf '%s\n' "${s//"$RPC"/"$RPC_SHOWN"}"
  done | redact_urls
}

# `bash -x` would print RPC_URL, key included, the moment it is expanded.
# Call before anything expands it; `${RPC_URL+set}` traces as `set`, never
# as the value.
refuse_xtrace_with_rpc_url() {
  case $- in
    *x*)
      if [ -n "${RPC_URL+set}" ]; then
        set +x
        echo "✗ refusing to run under bash -x while RPC_URL is set: the trace would print it, API key included (#97)" >&2
        exit 1
      fi ;;
  esac
}
