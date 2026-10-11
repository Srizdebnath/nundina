import type { Metadata } from "next";
import { Cinzel, Cormorant_Garamond, Plus_Jakarta_Sans, Space_Mono } from "next/font/google";
import "./globals.css";

const cinzel = Cinzel({ variable: "--ff-cinzel", subsets: ["latin"], weight: ["400", "500", "600", "700", "800", "900"] });
const cormorant = Cormorant_Garamond({
  variable: "--ff-cormorant",
  subsets: ["latin"],
  weight: ["300", "400", "500", "600", "700"],
  style: ["normal", "italic"],
});
const jakarta = Plus_Jakarta_Sans({ variable: "--ff-jakarta", subsets: ["latin"], weight: ["300", "400", "500", "600", "700"] });
const spaceMono = Space_Mono({ variable: "--ff-space-mono", subsets: ["latin"], weight: ["400", "700"] });

export const metadata: Metadata = {
  title: {
    default: "NUNDINA — Pricing, Queue & Risk-Transfer Engine for Private Credit",
    template: "%s · NUNDINA",
  },
  description:
    "NUNDINA is the pricing, queue & risk-transfer engine for gated, NAV-priced private credit on Solana. Devnet pilot — not audited, not an offer.",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html
      lang="en"
      className={`${cinzel.variable} ${cormorant.variable} ${jakarta.variable} ${spaceMono.variable} antialiased`}
    >
      <body className="bg-obsidian-900 text-gold-300">
        {children}
      </body>
    </html>
  );
}
