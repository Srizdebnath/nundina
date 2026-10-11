/**
 * Deployment address manifest — the single swap point for the dApp.
 *
 * Every program id, the pilot mint and the operator authorities the UI needs
 * are declared here as greppable tokens. A redeploy is one find-and-replace:
 *
 *     rg '__[A-Z0-9_]+__' app/lib/addresses.ts
 *
 * and paste the new base58 addresses over each token. Nothing else in the app
 * hardcodes a deployment address — `programs.ts` resolves every id through
 * `resolve()`, so the swap is complete when this file is complete.
 *
 * While a token is unresolved the app does NOT fail. `resolve()` falls back to
 * the id declared in the program IDL (which is what `anchor build` writes from
 * `declare_id!`) and `DEPLOYMENT_UNRESOLVED` goes true, so every page can show
 * "not deployed yet" instead of a decoding crash. There are no mock accounts,
 * no fixtures and no sample data anywhere in the read path: an unresolved id
 * simply resolves to whatever the IDL says, and the chain answers for itself.
 *
 * After a redeploy, `anchor build` regenerates `target/idl/*.json` with the new
 * `declare_id!` values and `npm run sync-idl` copies them into `app/idl/`, so
 * the IDL fallback and this manifest converge on the same addresses.
 */
import { address, type Address } from "@solana/kit";

/** `__TOKEN__` — an un-swapped placeholder. */
export const PLACEHOLDER_RE = /^__[A-Z0-9_]+__$/;

export const ADDRESSES = {
  // ── programs ────────────────────────────────────────────────────────────
  auction: "__AUCTION_ENGINE_PROGRAM__",
  gate: "__CREDIT_GATE_PROGRAM__",
  mark: "__MARK_ENGINE_PROGRAM__",
  queue: "__QUEUE_CLAIM_PROGRAM__",
  facility: "__LIQUIDITY_FACILITY_PROGRAM__",
  tranche: "__TRANCHE_ENGINE_PROGRAM__",
  treasury: "__NPCS_TREASURY_PROGRAM__",
  rails: "__PAYMENT_RAILS_PROGRAM__",

  // ── external programs we integrate, never deploy ────────────────────────
  sas: "__SAS_PROGRAM__",

  // ── pilot asset ─────────────────────────────────────────────────────────
  npcsMint: "__NPCS_MINT__",

  // ── authorities the UI reads to decide who may act ──────────────────────
  /** auction-engine `Venue` authority — the only role that may `open_session`. */
  venueAuthority: "__VENUE_AUTHORITY__",
  /** credit-gate `GateRoot` — the on-chain governance root. */
  gateRoot: "__GATE_ROOT_AUTHORITY__",
  /** payment-rails `RailsConfig` authority (upgrade, breaker, fees, grants). */
  railsAuthority: "__RAILS_AUTHORITY__",
  /** queue-claim `Queue.attester` — the issuer/transfer-agent bridge. */
  queueAttester: "__QUEUE_ATTESTER__",
  /** mark-engine `Feed.publishers[0]` — the only key that may `publish_nav`. */
  navPublisher: "__NAV_PUBLISHER__",
  /** Squads v4 vault holding program upgrade authority. */
  squadsVault: "__SQUADS_VAULT__",
  /** Token ACL `MintConfig` PDA — owns the NPCS freeze authority. */
  tokenAclMintConfig: "__TOKEN_ACL_MINT_CONFIG__",
} as const;

export type AddressKey = keyof typeof ADDRESSES;

export function isPlaceholder(v: string): boolean {
  return PLACEHOLDER_RE.test(v);
}

/** Manifest keys still holding a `__TOKEN__`. Empty means fully swapped in. */
export const UNRESOLVED_KEYS: AddressKey[] = (Object.keys(ADDRESSES) as AddressKey[]).filter((k) =>
  isPlaceholder(ADDRESSES[k]),
);

/** True while any address is a placeholder — pages render a banner, not a crash. */
export const DEPLOYMENT_UNRESOLVED = UNRESOLVED_KEYS.length > 0;

/**
 * Resolve a manifest key to a base58 address.
 *
 * `fallback` is the id declared in the program IDL. It is used verbatim while
 * the manifest still holds a placeholder, so the app keeps reading the chain
 * correctly instead of throwing on a non-base58 token.
 */
export function resolve(key: AddressKey, fallback: Address): Address {
  const v = ADDRESSES[key];
  if (isPlaceholder(v)) return fallback;
  try {
    return address(v);
  } catch {
    // A pasted value that is not valid base58 is a swap bug, not a runtime one:
    // fall back rather than take the whole app down, and leave the key flagged.
    return fallback;
  }
}

/** Resolve without an IDL fallback — for keys that have no program to read from. */
export function resolveOrZero(key: AddressKey, zero: Address): Address {
  return resolve(key, zero);
}
