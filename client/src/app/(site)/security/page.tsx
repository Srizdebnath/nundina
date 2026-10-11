import type { Metadata } from "next";
import Link from "next/link";
import { Band, Cards, PageHero, Prose } from "@/components/page/PageHero";

export const metadata: Metadata = {
  title: "Security",
  description: "NUNDINA's security posture: what is live, what is verifiable, and what is still pending.",
};

export default function SecurityPage() {
  return (
    <>
      <PageHero eyebrow="Security" title="Security Posture, Stated Plainly">
        What is live, what anyone can verify, and what is still pending. NUNDINA has not been audited.
      </PageHero>
      <Band eyebrow="Controls" title="What protects the pilot today">
        <Cards
          items={[
            { title: "Squads 2-of-2 + 48h", body: "payment-rails and credit-gate upgrades and the NPCS mint authorities sit behind a Squads v4 multisig with a 48-hour timelock. Devnet caveats: both member keys were generated on one machine, and the engine programs still upgrade from a single deployer key." },
            { title: "Bounded operator", body: "The operator can choose a matching but cannot redirect any leg; pinned by the test operator_cannot_redirect_any_leg. The protocol fee cap is 0 bps." },
            { title: "Compliance at activation", body: "NPCS accounts are born frozen. Only credit-gate, through Token ACL, can thaw an account, after reading the holder's SAS attestation." },
            { title: "Conservation checks", body: "Every fill checks to_seller + fee + to_reserve == gross in u128 through credit-math, against real token balances." },
            { title: "Keyless verification", body: "Deployed program dumps are hashed and matched to recorded build hashes; the verify scripts need no keys." },
            { title: "Stale-mark guards", body: "Engines check the mark state (Fresh / Warned / Stale) before pricing, advancing or allocating." },
          ]}
        />
      </Band>
      <Band eyebrow="Pending" title="Before any mainnet use">
        <Prose>
          <ul>
            <li><strong>No third-party audit has been performed.</strong> An external audit, with criticals and highs fixed and re-reviewed, is a hard precondition for mainnet.</li>
            <li>Counsel memos on the offering gate and on USDC movement must clear first.</li>
            <li>A fresh mainnet NPCS mint is created in a key ceremony; the devnet mint is never reused.</li>
            <li>Devnet keys, including the multisig members, are development keys and are never reused for mainnet.</li>
          </ul>
          <p>
            Found a vulnerability? Open a private security advisory on the{" "}
            <a href="https://github.com/Srizdebnath/nundina/security/advisories/new" target="_blank" rel="noreferrer">GitHub repository</a>. Full risk and audit scope:{" "}
            <Link href="/docs/15-legal-risk-audit-roadmap">Legal, Risk, Audit & Roadmap</Link>.
          </p>
        </Prose>
      </Band>
    </>
  );
}
