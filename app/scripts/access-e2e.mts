/**
 * End to end for the private allowlist against a running app (SRS 08 §7
 * Access), with real wallets signing real challenges:
 *
 *   1. every page answers with the sign-in gate, not data, to a stranger;
 *   2. a fresh wallet signs its challenge correctly and is still refused
 *      (403: credit-gate `validate` rejects it);
 *   3. a wallet the gate admits (devnet holder H, HOLDER key) signs in through
 *      credit-gate, or an operator wallet through NUNDINA_ALLOWLIST (OPERATOR
 *      key); its cookie then opens /admin and /data-room with chain data;
 *   4. a signature over someone else's challenge is refused (401).
 *
 *   NUNDINA_ACCESS=allowlist NUNDINA_ACCESS_SECRET=… npm run start -- -p 3123 &
 *   BASE=http://127.0.0.1:3123 HOLDER=~/.config/nundina/npcs-devnet/holder-h.json npm run access:e2e
 */
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { createKeyPairFromBytes, generateKeyPair, getAddressFromPublicKey, signBytes, type Address } from "@solana/kit";

const BASE = process.env.BASE ?? "http://127.0.0.1:3123";
const keyPath = process.env.HOLDER ?? process.env.OPERATOR;
if (!keyPath) throw new Error("set HOLDER (a gate-admitted wallet's keypair JSON) or OPERATOR (one on NUNDINA_ALLOWLIST)");

const b64 = (b: Uint8Array) => Buffer.from(b).toString("base64");
const check = (ok: boolean, what: string) => {
  if (!ok) throw new Error(`FAIL ${what}`);
  console.log(`✓ ${what}`);
};

async function wallet(kp: CryptoKeyPair) {
  const address = await getAddressFromPublicKey(kp.publicKey);
  return { address, sign: async (m: string) => new Uint8Array(await signBytes(kp.privateKey, new TextEncoder().encode(m))) };
}

async function signIn(w: { address: Address; sign: (m: string) => Promise<Uint8Array> }, as: Address = w.address) {
  const { message } = (await (await fetch(`${BASE}/api/access?wallet=${as}`)).json()) as { message: string };
  const r = await fetch(`${BASE}/api/access`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ wallet: w.address, message, signature: b64(await w.sign(message)) }),
  });
  return { status: r.status, body: (await r.json()) as { error?: string; via?: string }, cookie: r.headers.get("set-cookie")?.split(";")[0] ?? null };
}

const page = async (path: string, cookie?: string) => (await fetch(`${BASE}${path}`, { headers: cookie ? { cookie } : {} })).text();

const GATE = "Private allowlist";

for (const p of ["/", "/admin", "/data-room", "/portfolio"]) check((await page(p)).includes(GATE), `${p} shows the sign-in gate to a stranger`);

const stranger = await wallet(await generateKeyPair());
const s = await signIn(stranger);
check(s.status === 403 && /credit-gate validate rejects/.test(s.body.error ?? ""), `fresh wallet refused: ${s.body.error}`);

const member = await wallet(await createKeyPairFromBytes(Uint8Array.from(JSON.parse(readFileSync(keyPath.replace(/^~/, homedir()), "utf8")))));
const replay = await signIn(stranger, member.address);
check(replay.status === 401, `signature over another wallet's challenge refused: ${replay.body.error}`);

const m = await signIn(member);
check(m.status === 200 && !!m.cookie, `${member.address} admitted via ${m.body.via}`);

const admin = await page("/admin", m.cookie!);
check(!admin.includes(GATE) && admin.includes("Authority map") && admin.includes("payment-rails config authority"), "/admin renders the authority map for the viewer");
check(admin.includes("Squads vault"), "/admin labels the Squads vault");
const room = await page("/data-room", m.cookie!);
check(!room.includes(GATE) && room.includes("Data room"), "/data-room opens for the viewer");
check((await page("/admin", `${m.cookie!.split("=")[0]}=x.1.y`)).includes(GATE), "a forged cookie gets the gate");
console.log("ACCESS E2E OK");
