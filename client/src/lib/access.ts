/**
 * The private allowlist (SRS 08 §7 Access): "connect + CG check against
 * market policy before any page data; read access only for allowlisted
 * wallets". Server-side only.
 *
 * 1. The page asks for a challenge for a wallet: a message naming the
 *    wallet, the cluster, the time and a nonce MAC'd with the server
 *    secret (stateless, so no session store).
 * 2. The wallet signs it (`solana:signMessage`); the server checks the
 *    ed25519 signature, the MAC, the wallet and a 5-minute age.
 * 3. The wallet is admitted if it is on `NUNDINA_ALLOWLIST` (operators,
 *    the team) or credit-gate's own `validate`, simulated against the NPCS
 *    market, approves it and it is not block-listed: the same decision the
 *    `/gate` page shows.
 * 4. The answer is a MAC'd, httpOnly cookie naming the wallet, for 12 hours.
 *
 * `NUNDINA_ACCESS=allowlist` gates every page; otherwise (local
 * development) only `/data-room` and `/admin` are gated. In allowlist mode a missing
 * `NUNDINA_ACCESS_SECRET` fails closed.
 */
import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { cookies } from "next/headers";
import { connection } from "next/server";
import { address, getPublicKeyFromAddress, isAddress, signatureBytes, verifySignature, type Address } from "@solana/kit";
import { gateStatus } from "./gate";
import { CLUSTER, NPCS_MINT } from "./programs";

export const ACCESS_COOKIE = "nundina_access";
export const CHALLENGE_TTL = 5 * 60;
export const ACCESS_TTL = 12 * 3600;

export const accessMode = (): "allowlist" | "open" => (process.env.NUNDINA_ACCESS === "allowlist" ? "allowlist" : "open");

/** A per-process secret is acceptable only in open (development) mode. */
const devSecret = randomBytes(32).toString("hex");
function secret(): string {
  const s = process.env.NUNDINA_ACCESS_SECRET;
  if (s && s.length >= 32) return s;
  if (accessMode() === "allowlist") throw new Error("NUNDINA_ACCESS_SECRET (≥ 32 chars) is required with NUNDINA_ACCESS=allowlist");
  return devSecret;
}

const mac = (s: string) => createHmac("sha256", secret()).update(s).digest("hex");
function macOk(s: string, m: string): boolean {
  const want = Buffer.from(mac(s), "hex");
  const got = Buffer.from(m, "hex");
  return got.length === want.length && timingSafeEqual(got, want);
}

export function staticAllowlist(): Set<string> {
  return new Set((process.env.NUNDINA_ALLOWLIST ?? "").split(/[\s,]+/).filter((w) => w && isAddress(w)));
}

export function challenge(wallet: Address, now = Math.floor(Date.now() / 1000)): string {
  const nonce = randomBytes(16).toString("hex");
  const body = `${wallet}|${CLUSTER}|${now}|${nonce}`;
  return [
    "NUNDINA pilot: read access",
    `wallet: ${wallet}`,
    `cluster: ${CLUSTER}`,
    `issued: ${now}`,
    `nonce: ${nonce}.${mac(body)}`,
    "Signing proves you hold this wallet. It sends nothing and costs nothing.",
  ].join("\n");
}

/** Parse and check a signed challenge; returns the wallet or a reason. */
export async function verifyChallenge(
  wallet: string,
  message: string,
  signature: Uint8Array,
  now = Math.floor(Date.now() / 1000),
): Promise<{ wallet: Address } | { reason: string }> {
  if (!isAddress(wallet)) return { reason: "not an address" };
  const f = Object.fromEntries(
    message
      .split("\n")
      .map((l) => l.match(/^(wallet|cluster|issued|nonce): (.+)$/))
      .filter((m): m is RegExpMatchArray => m !== null)
      .map((m) => [m[1]!, m[2]!]),
  );
  const [nonce, m] = (f.nonce ?? "").split(".");
  if (f.wallet !== wallet || f.cluster !== CLUSTER || !nonce || !m) return { reason: "challenge does not match this wallet or cluster" };
  const issued = Number(f.issued);
  if (!Number.isInteger(issued) || now - issued > CHALLENGE_TTL || issued > now + 30) return { reason: "challenge expired: ask for a new one" };
  if (!macOk(`${wallet}|${CLUSTER}|${issued}|${nonce}`, m)) return { reason: "challenge was not issued by this server" };
  if (signature.length !== 64) return { reason: "signature must be 64 bytes" };
  const key = await getPublicKeyFromAddress(address(wallet));
  const ok = await verifySignature(key, signatureBytes(signature), new TextEncoder().encode(message));
  return ok ? { wallet: address(wallet) } : { reason: "signature does not verify for this wallet" };
}

/** The CG check: allowlisted operator, or credit-gate `validate` approves and no block entry. */
export async function admission(wallet: Address): Promise<{ admitted: boolean; via: string }> {
  if (staticAllowlist().has(wallet)) return { admitted: true, via: "operator allowlist (NUNDINA_ALLOWLIST)" };
  const g = await gateStatus(NPCS_MINT, wallet);
  if (g.blocked) return { admitted: false, via: "credit-gate: block-listed" };
  const d = g.decision;
  if (d.reject) return { admitted: false, via: `credit-gate validate rejects: ${d.reject}` };
  if (d.error) return { admitted: false, via: `credit-gate validate: ${d.error}` };
  return { admitted: true, via: "credit-gate validate approves" };
}

export function accessToken(wallet: Address, now = Math.floor(Date.now() / 1000)): string {
  const exp = now + ACCESS_TTL;
  return `${wallet}.${exp}.${mac(`access|${wallet}|${CLUSTER}|${exp}`)}`;
}

export function readAccessToken(token: string | undefined, now = Math.floor(Date.now() / 1000)): Address | null {
  if (!token) return null;
  const [w, e, m] = token.split(".");
  if (!w || !e || !m || !isAddress(w)) return null;
  const exp = Number(e);
  if (!Number.isInteger(exp) || exp <= now) return null;
  return macOk(`access|${w}|${CLUSTER}|${exp}`, m) ? address(w) : null;
}

/** The signed-in viewer of this request, if any. */
export async function viewer(): Promise<Address | null> {
  const token = (await cookies()).get(ACCESS_COOKIE)?.value;
  await connection(); // the expiry check reads the clock: request time only
  return readAccessToken(token);
}
