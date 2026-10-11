"use client";
/** The allowlist sign-in: connect, sign the server's challenge, reload as an admitted viewer. */
import React, { useState } from "react";
import { useRouter } from "next/navigation";
import { toBase64 } from "@/lib/borsh";
import { useWallet } from "@/lib/wallet";

export function AccessGate({ what }: { what: string }) {
  const { account, signMessage } = useWallet();
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const signIn = async () => {
    if (!account) return;
    setBusy(true);
    setErr(null);
    try {
      const c = await fetch(`/api/access?wallet=${account}`);
      const { message, error } = (await c.json()) as { message?: string; error?: string };
      if (!message) throw new Error(error ?? "no challenge");
      const signature = await signMessage(new TextEncoder().encode(message));
      const r = await fetch("/api/access", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ wallet: account, message, signature: toBase64(signature) }),
      });
      const out = (await r.json()) as { error?: string };
      if (!r.ok) throw new Error(out.error ?? `HTTP ${r.status}`);
      router.refresh();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="panel" style={{ maxWidth: 640 }}>
      <h2 style={{ marginTop: 0 }}>Private allowlist</h2>
      <p className="small">
        {what} is visible only to allowlisted wallets: operators, or wallets credit-gate admits to the NPCS market (its own{" "}
        <code>validate</code>, the decision the gate check page shows). Sign a short message to prove you hold the wallet. Nothing is sent
        on-chain.
      </p>
      {account ? (
        <button disabled={busy} onClick={() => void signIn()}>
          {busy ? "checking…" : `sign in as ${account.slice(0, 4)}…${account.slice(-4)}`}
        </button>
      ) : (
        <p className="small muted">Connect a wallet (top right) to sign in.</p>
      )}
      {err && <p className="small bad">{err}</p>}
    </div>
  );
}

export function SignOut() {
  const router = useRouter();
  return (
    <button
      className="ghost"
      onClick={() => void fetch("/api/access", { method: "DELETE" }).then(() => router.refresh())}
    >
      sign out
    </button>
  );
}
