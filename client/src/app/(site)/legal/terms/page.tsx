import type { Metadata } from "next";
import { Band, PageHero, Prose } from "@/components/page/PageHero";

export const metadata: Metadata = { title: "Terms of Use", description: "Terms for using the NUNDINA devnet pilot website and app." };

export default function TermsPage() {
  return (
    <>
      <PageHero eyebrow="Legal" title="Terms of Use">
        Plain-language terms for a devnet pilot. Not reviewed by counsel.
      </PageHero>
      <Band title="Pilot notice">
        <Prose>
          <p><strong>PILOT-DEMO-ASSET — NOT AN OFFER.</strong> NUNDINA runs on Solana devnet. Nothing on this site is an offer to sell, or a solicitation to buy, any security or financial product, and nothing here is investment, legal or tax advice.</p>
          <p>Devnet tokens, including NPCS and devnet USDC, have no monetary value. Do not send mainnet assets to any address shown here.</p>
          <p>The software is unaudited and provided &ldquo;as is&rdquo;, without warranty of any kind. Programs, accounts and data may be changed, reset or removed at any time.</p>
          <p>You are responsible for your wallet, its keys, and every transaction you sign. The app builds transactions in your browser and never holds your keys.</p>
          <p>The source code is published under the license in the project repository.</p>
        </Prose>
      </Band>
    </>
  );
}
