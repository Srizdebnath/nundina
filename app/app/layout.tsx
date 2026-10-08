import type { Metadata } from "next";
import React, { Suspense } from "react";
import Link from "next/link";
import "./globals.css";
import { WalletProvider } from "@/lib/wallet";
import { WalletButton } from "@/components/client";
import { CLUSTER, RPC_URL } from "@/lib/programs";

export const metadata: Metadata = {
  title: "NUNDINA — Pilot",
  description:
    "Pricing, queue & risk-transfer engine for gated, NAV-priced private credit. PILOT-DEMO-ASSET — NOT AN OFFER.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
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
            <Link href="/gate">Gate check</Link>
            <span className="spacer" />
            <span className="small muted" title={RPC_URL}>
              {CLUSTER}
            </span>
            <WalletButton />
          </nav>
          <Suspense>{children}</Suspense>
        </WalletProvider>
      </body>
    </html>
  );
}
