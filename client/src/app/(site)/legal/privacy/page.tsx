import type { Metadata } from "next";
import { Band, PageHero, Prose } from "@/components/page/PageHero";

export const metadata: Metadata = { title: "Privacy", description: "What the NUNDINA pilot site does and does not collect." };

export default function PrivacyPage() {
  return (
    <>
      <PageHero eyebrow="Legal" title="Privacy">
        What this site processes, stated plainly. Not reviewed by counsel.
      </PageHero>
      <Band title="What we process">
        <Prose>
          <ul>
            <li><strong>No accounts, no analytics, no tracking cookies.</strong> The site does not ask for your name or email.</li>
            <li><strong>Wallet address.</strong> When you connect a wallet, its public address is used to read on-chain state and build transactions in your browser. Your browser remembers which wallet you used last so it can reconnect.</li>
            <li><strong>Blockchain reads.</strong> Pages query a Solana RPC endpoint (public devnet by default). That provider sees the addresses being looked up. Everything written on-chain is public and permanent.</li>
            <li><strong>Gated pages.</strong> Signing in to the data room or admin console sets one httpOnly cookie that names your wallet and expires after 12 hours. The operator may log the wallet address and time of visits to those pages.</li>
            <li><strong>Hosting.</strong> The site is served by Vercel behind Cloudflare, whose standard request logs (such as IP address and user agent) are governed by their own policies.</li>
          </ul>
        </Prose>
      </Band>
    </>
  );
}
