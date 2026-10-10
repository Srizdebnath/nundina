import type { Metadata } from "next";
import React, { Suspense } from "react";
import Link from "next/link";
import "./globals.css";
import { WalletProvider } from "@/lib/wallet";
import { WalletButton } from "@/components/client";
import { AccessGuard } from "@/components/AccessGuard";
import { SignOut } from "@/components/AccessGate";
import { viewer } from "@/lib/access";
import { CLUSTER, RPC_URL } from "@/lib/programs";

export const metadata: Metadata = {
  title: "NUNDINA — Pilot",
  description:
    "Pricing, queue & risk-transfer engine for gated, NAV-priced private credit. PILOT-DEMO-ASSET — NOT AN OFFER.",
};

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  const me = await viewer();
  return (
    <html lang="en">
      <body>
        <WalletProvider>
          <nav className="top">
            <Link href="/" className="brand">
              NUNDINA
            </Link>
            <Link href="/sessions">Sessions</Link>
            <Link href="/portfolio">Portfolio</Link>
            <Link href="/queue">Queue</Link>
            <Link href="/facility">Facility</Link>
            <Link href="/tranches">Tranches</Link>
            <Link href="/treasury">Treasury</Link>
            <Link href="/gate">Gate check</Link>
            <Link href="/data-room">Data room</Link>
            <Link href="/admin">Admin</Link>
            <span className="spacer" />
            <span className="small muted" title={RPC_URL}>
              {CLUSTER}
            </span>
            {me && (
              <span className="small muted" title={me}>
                viewer {me.slice(0, 4)}…{me.slice(-4)} <SignOut />
              </span>
            )}
            <WalletButton />
          </nav>
          <Suspense>
            <AccessGuard>{children}</AccessGuard>
          </Suspense>
        </WalletProvider>
      </body>
    </html>
  );
}
