/**
 * NPCS mint verification — asserts doc 09 §2 (extension manifest) and
 * §2.1 (provenance) against a live mint. Exit 1 on any failure.
 *
 * Architecture: Token-2022 self-pointer — metadata TLV lives in the mint,
 * so `fetchMint` parses everything (no separate metadata account).
 *
 * Usage: CLUSTER=devnet MINT=<address> npm run verify
 */
import {
  address,
  createSolanaRpc,
  getUtf8Encoder,
} from "@solana/kit";
import {
  AccountState,
  fetchMint,
  TOKEN_2022_PROGRAM_ADDRESS,
} from "@solana-program/token-2022";
import { readFile } from "node:fs/promises";

const CLUSTER = (process.env.CLUSTER ?? "devnet") as "devnet" | "mainnet";
const RPC_URL =
  process.env.RPC_URL ??
  (CLUSTER === "devnet"
    ? "https://api.devnet.solana.com"
    : "https://api.mainnet-beta.solana.com");

let failures = 0;
function check(ok: boolean, label: string, detail = ""): void {
  console.log(`${ok ? "  ✓" : "  ✗"} ${label}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures++;
}

// kit Option<T> unwrapping: { __option: "Some", value } | { __option: "None" }
function unwrapOpt(o: unknown): string | null {
  const x = o as { __option?: string; value?: unknown } | null | undefined;
  return x && x.__option === "Some" ? String(x.value) : null;
}
function asPairs(v: unknown): [string, string][] {
  if (v instanceof Map) return Array.from(v.entries()) as [string, string][];
  if (Array.isArray(v)) return v as [string, string][];
  const obj = v as Record<string, string> | null | undefined;
  return obj ? Object.entries(obj) : [];
}

async function main(): Promise<void> {
  const deployment = JSON.parse(
    await readFile(`keys/deployment-${CLUSTER}.json`, "utf8"),
  ) as { mint: string; admin_authority: string };
  const mintAddress = process.env.MINT ?? deployment.mint;
  const admin = deployment.admin_authority;

  const rpc = createSolanaRpc(RPC_URL);
  console.log(`\nverify: ${mintAddress} on ${CLUSTER}\n`);

  const mint = await fetchMint(rpc, address(mintAddress));
  check(mint.programAddress === TOKEN_2022_PROGRAM_ADDRESS, "owned by Token-2022", mint.programAddress);
  check(mint.data.isInitialized, "initialized");
  check(mint.data.decimals === 6, "decimals == 6", String(mint.data.decimals));

  const mintAuth = unwrapOpt(mint.data.mintAuthority);
  const freezeAuth = unwrapOpt(mint.data.freezeAuthority);
  check(mintAuth === admin, "mint authority == admin", mintAuth ?? "none");
  check(freezeAuth === admin, "freeze authority == admin (bootstrap)", freezeAuth ?? "none");

  const extOpt = mint.data.extensions;
  const exts = (extOpt && extOpt.__option === "Some" ? extOpt.value : []) as unknown as Array<{
    __kind: string;
    [k: string]: unknown;
  }>;
  const byKind = (k: string) => exts.find((e) => e.__kind === k);

  // ── §2 manifest: the three required extensions ──────────────────────
  const das = byKind("DefaultAccountState") as { state?: AccountState } | undefined;
  check(!!das, "extension: DefaultAccountState present");
  check(das?.state === AccountState.Frozen, "default state == Frozen");

  const pd = byKind("PermanentDelegate") as { delegate?: { toString(): string } } | undefined;
  check(!!pd, "extension: PermanentDelegate present");
  check(pd?.delegate?.toString() === admin, "delegate == admin", pd?.delegate?.toString());

  const mp = byKind("MetadataPointer") as { metadataAddress?: unknown } | undefined;
  check(!!mp, "extension: MetadataPointer present");
  check(unwrapOpt(mp?.metadataAddress) === mintAddress, "pointer is SELF (metadata in mint)", unwrapOpt(mp?.metadataAddress) ?? "none");

  // ── TokenMetadata TLV (parsed by fetchMint) ─────────────────────────
  const tm = byKind("TokenMetadata") as
    | {
        name?: string;
        symbol?: string;
        uri?: string;
        updateAuthority?: unknown;
        additionalMetadata?: unknown;
      }
    | undefined;
  check(!!tm, "extension: TokenMetadata present");
  if (tm) {
    check(unwrapOpt(tm.updateAuthority) === admin, "metadata.updateAuthority == admin", unwrapOpt(tm.updateAuthority) ?? "none");
    check((tm.name ?? "").includes("NUNDINA Pilot Credit Sleeve"), "name ok", tm.name);
    check(tm.symbol === "NPCS", "symbol ok", tm.symbol);
    check((tm.uri ?? "").length > 0, "uri present", tm.uri);
    const labels = asPairs(tm.additionalMetadata);
    check(
      labels.some(([k, v]) => k === "label" && v.includes("NOT AN OFFER")),
      "NOT AN OFFER label present on-chain",
      labels.find(([k]) => k === "label")?.[1],
    );
    check(labels.some(([k, v]) => k === "backing" && v === "USDC_1to1"), "backing label ok");
    check(labels.some(([k, v]) => k === "nav_precision" && v === "pico"), "nav_precision label ok");
  }

  // ── §2 forbidden manifest (what NPCS must NOT carry) ────────────────
  const forbidden = [
    "TransferHook",
    "TransferFeeConfig",
    "InterestBearingConfig",
    "ConfidentialTransferMint",
    "NonTransferable",
    "GroupPointer",
    // Not in the §2 manifest: a close authority could delete the mint once
    // supply hits zero and let a new mint reuse the address.
    "MintCloseAuthority",
  ];
  for (const k of forbidden) {
    check(!byKind(k), `forbidden absent: ${k}`);
  }

  // ── §2.1 provenance (INFORMATIONAL until ACL wiring) ────────────────
  console.log("\n§2.1 provenance:");
  const aclProgram = "TACLkU6aswbpgiUF6dMuVHnVp3S6oBxDiWzN5iJebFu";
  console.log(
    `  · freeze authority ${freezeAuth} is wallet-held (bootstrap). After Token ACL ` +
      `wiring this becomes a REQUIRED check: authority account owner == ${aclProgram}`,
  );

  console.log(`\nresult: ${failures === 0 ? "ALL CHECKS PASSED" : `${failures} CHECK(S) FAILED`}`);
  process.exit(failures === 0 ? 0 : 1);
}

void address;
void getUtf8Encoder;
void TOKEN_2022_PROGRAM_ADDRESS;

main().catch((e) => {
  console.error("verify failed:", e);
  process.exit(1);
});
