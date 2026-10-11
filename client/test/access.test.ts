import { afterEach, describe, expect, it } from "vitest";
import { generateKeyPair, getAddressFromPublicKey, signBytes } from "@solana/kit";
import { accessToken, admission, challenge, CHALLENGE_TTL, readAccessToken, verifyChallenge } from "@/lib/access";

async function signer() {
  const kp = await generateKeyPair();
  const wallet = await getAddressFromPublicKey(kp.publicKey);
  const sign = async (m: string) => new Uint8Array(await signBytes(kp.privateKey, new TextEncoder().encode(m)));
  return { wallet, sign };
}

describe("allowlist sign-in", () => {
  afterEach(() => {
    delete process.env.NUNDINA_ALLOWLIST;
  });

  it("accepts the wallet's own signature over a fresh challenge", async () => {
    const { wallet, sign } = await signer();
    const m = challenge(wallet);
    expect(await verifyChallenge(wallet, m, await sign(m))).toEqual({ wallet });
  });

  it("rejects another wallet's signature, an edited or foreign challenge, and an old one", async () => {
    const a = await signer();
    const b = await signer();
    const m = challenge(a.wallet);
    expect(await verifyChallenge(a.wallet, m, await b.sign(m))).toHaveProperty("reason");
    const edited = m.replace(/issued: (\d+)/, (_x, t) => `issued: ${Number(t) + 1}`);
    expect(await verifyChallenge(a.wallet, edited, await a.sign(edited))).toEqual({ reason: "challenge was not issued by this server" });
    const forged = m.replace(/nonce: (\w+)\.\w+/, "nonce: $1." + "0".repeat(64));
    expect(await verifyChallenge(a.wallet, forged, await a.sign(forged))).toEqual({ reason: "challenge was not issued by this server" });
    // b replaying a's challenge under its own name
    expect(await verifyChallenge(b.wallet, m, await b.sign(m))).toHaveProperty("reason");
    const old = challenge(a.wallet, Math.floor(Date.now() / 1000) - CHALLENGE_TTL - 1);
    expect(await verifyChallenge(a.wallet, old, await a.sign(old))).toEqual({ reason: "challenge expired: ask for a new one" });
  });

  it("issues a cookie token that names the wallet, expires, and cannot be forged", async () => {
    const { wallet } = await signer();
    const other = (await signer()).wallet;
    const t = accessToken(wallet);
    expect(readAccessToken(t)).toBe(wallet);
    expect(readAccessToken(t, Math.floor(Date.now() / 1000) + 13 * 3600)).toBeNull();
    const [, exp, mac] = t.split(".");
    expect(readAccessToken(`${other}.${exp}.${mac}`)).toBeNull();
    expect(readAccessToken(`${wallet}.${Number(exp) + 3600}.${mac}`)).toBeNull();
    expect(readAccessToken(undefined)).toBeNull();
    expect(readAccessToken("garbage")).toBeNull();
  });

  it("admits operator wallets from NUNDINA_ALLOWLIST without a gate read", async () => {
    const { wallet } = await signer();
    process.env.NUNDINA_ALLOWLIST = `x, ${wallet}`;
    expect(await admission(wallet)).toEqual({ admitted: true, via: "operator allowlist (NUNDINA_ALLOWLIST)" });
  });
});
