import type { Metadata } from "next";
import React, { Suspense } from "react";
import Link from "next/link";
import "./dapp.css";
import { WalletProvider } from "@/lib/wallet";
import { WalletButton } from "@/components/app/client";
import { SignOut } from "@/components/app/AccessGate";
import { AppNav, AppNavStatic } from "@/components/app/AppNav";
import { DeploymentBanner } from "@/components/app/DeploymentBanner";
import { LogoMark } from "@/components/ui/Logo";
import { viewer } from "@/lib/access";
import { CLUSTER, RPC_URL } from "@/lib/programs";

export const metadata: Metadata = {
  title: { default: "App", template: "%s · NUNDINA App" },
  description:
    "Live NUNDINA pilot on Solana devnet: sealed-bid sessions, gate checks, portfolio, queue, facility, tranches and treasury. PILOT-DEMO-ASSET — NOT AN OFFER.",
};

/** The signed-in allowlist viewer, if any (reads the access cookie, so it streams). */
async function Viewer() {
  const me = await viewer();
  if (!me) return null;
  return (
    <span className="small muted viewer" title={me}>
      signed in {me.slice(0, 4)}…{me.slice(-4)} <SignOut />
    </span>
  );
}

function Loading() {
  return (
    <main className="wrap">
      <div className="skeleton" style={{ width: 220, height: 14 }} />
      <div className="skeleton" style={{ width: 320, height: 28, marginTop: 14 }} />
      <div className="grid" style={{ marginTop: 24 }}>
        {Array.from({ length: 4 }, (_, i) => (
          <div key={i} className="panel skeleton" style={{ height: 76 }} />
        ))}
      </div>
      <p className="small muted" style={{ marginTop: 16 }}>
        Reading {CLUSTER}…
      </p>
    </main>
  );
}

export default function AppLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="dapp">
      <WalletProvider>
        <header className="app-header">
          <div className="app-bar">
            <Link href="/" className="brand" aria-label="NUNDINA home">
              <LogoMark className="h-7 w-7" />
              <span>NUNDINA</span>
            </Link>
            <span className="pill cluster" title={RPC_URL}>
              <span className="dot" /> {CLUSTER}
            </span>
            <span className="spacer" />
            <Suspense>
              <Viewer />
            </Suspense>
            <WalletButton />
          </div>
          <Suspense fallback={<AppNavStatic />}>
            <AppNav />
          </Suspense>
        </header>
        <DeploymentBanner />
        {/* Read is public. /app/admin and /app/data-room carry their own
            `AccessGuard always`. Writes always require a connected wallet,
            signed client-side; the server never signs. */}
        <Suspense fallback={<Loading />}>{children}</Suspense>
        <footer className="app-footer small muted">
          PILOT-DEMO-ASSET — NOT AN OFFER · Solana {CLUSTER} · not audited ·{" "}
          <Link href="/docs">docs</Link> · <Link href="/deployments">deployments</Link>
        </footer>
      </WalletProvider>
    </div>
  );
}
