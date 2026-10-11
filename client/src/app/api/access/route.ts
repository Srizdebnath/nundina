/**
 * The allowlist sign-in (lib/access.ts): GET a challenge for a wallet, POST
 * it back signed, DELETE to sign out. Read access only: nothing here signs
 * or sends a transaction.
 */
import { NextResponse, type NextRequest } from "next/server";
import { address, isAddress } from "@solana/kit";
import { fromBase64 } from "@/lib/borsh";
import { ACCESS_COOKIE, ACCESS_TTL, accessToken, admission, challenge, verifyChallenge, viewer } from "@/lib/access";
import { logAccess } from "@/lib/access-log";


export function GET(req: NextRequest) {
  const w = req.nextUrl.searchParams.get("wallet") ?? "";
  if (!isAddress(w)) return NextResponse.json({ error: "wallet must be a base58 address" }, { status: 400 });
  logAccess("challenge", w, "/api/access");
  return NextResponse.json({ message: challenge(address(w)) });
}

export async function POST(req: NextRequest) {
  let body: { wallet?: string; message?: string; signature?: string };
  try {
    body = (await req.json()) as typeof body;
  } catch {
    return NextResponse.json({ error: "JSON body required" }, { status: 400 });
  }
  if (!body.wallet || !body.message || !body.signature) {
    return NextResponse.json({ error: "wallet, message and signature (base64) are required" }, { status: 400 });
  }
  let sig: Uint8Array;
  try {
    sig = fromBase64(body.signature);
  } catch {
    return NextResponse.json({ error: "signature is not base64" }, { status: 400 });
  }
  const v = await verifyChallenge(body.wallet, body.message, sig);
  if ("reason" in v) {
    logAccess("sign-in-refused", isAddress(body.wallet) ? body.wallet : null, "/api/access", v.reason);
    return NextResponse.json({ error: v.reason }, { status: 401 });
  }
  const a = await admission(v.wallet);
  if (!a.admitted) {
    logAccess("sign-in-refused", v.wallet, "/api/access", a.via);
    return NextResponse.json({ error: `not allowlisted: ${a.via}` }, { status: 403 });
  }
  logAccess("sign-in", v.wallet, "/api/access", a.via);
  const res = NextResponse.json({ wallet: v.wallet, via: a.via });
  res.cookies.set(ACCESS_COOKIE, accessToken(v.wallet), {
    httpOnly: true,
    sameSite: "lax",
    secure: process.env.NODE_ENV === "production",
    path: "/",
    maxAge: ACCESS_TTL,
  });
  return res;
}

export async function DELETE() {
  logAccess("sign-out", await viewer(), "/api/access");
  const res = NextResponse.json({ ok: true });
  res.cookies.delete(ACCESS_COOKIE);
  return res;
}
