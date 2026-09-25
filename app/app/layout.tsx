import type { Metadata } from "next";
import React from "react";

export const metadata: Metadata = {
  title: "NUNDINA — Pilot",
  description:
    "Pricing, queue & risk-transfer engine for gated, NAV-priced private credit. PILOT-DEMO-ASSET — NOT AN OFFER.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body
        style={{
          margin: 0,
          background: "#0b0e14",
          color: "#d7dde8",
          fontFamily:
            "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace",
          minHeight: "100vh",
        }}
      >
        {children}
      </body>
    </html>
  );
}
