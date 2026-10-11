import type { Metadata } from "next";
import Link from "next/link";
import { Band, Cards, PageHero, Prose } from "@/components/page/PageHero";
import { REGISTRY } from "@/lib/registry";

export const metadata: Metadata = {
  title: "Architecture",
  description: "Eight Anchor programs, one shared math crate, and the trust flow between them.",
};

export default function ArchitecturePage() {
  return (
    <>
      <PageHero eyebrow="Architecture" title="Eight Programs, One Math Crate">
        Each primitive is its own Anchor program. They share fixed-point arithmetic through <code>credit-math</code> and move value only through the payment-rails cash layer.
      </PageHero>
      <Band eyebrow="Program map" title="The eight programs">
        <Cards
          items={REGISTRY.map((r) => ({
            meta: r.name,
            title: r.name.replace("-", " "),
            body: (
              <>
                {r.role}. <Link className="text-gold-200 underline underline-offset-4" href={`/docs/${r.doc}`}>Docs →</Link>
              </>
            ),
          }))}
        />
      </Band>
      <Band eyebrow="Layers" title="How it fits together">
        <Prose>
          <ul>
            <li><strong>Asset & compliance.</strong> NPCS is a default-frozen Token-2022 mint. Token ACL routes thaw decisions to credit-gate, which reads SAS attestations and enforces the holder cap, lot seasoning and the block-list.</li>
            <li><strong>Pricing.</strong> mark-engine publishes NAV; auction-engine runs sealed-bid sessions against the mark taken at session open and publishes the clearing print back.</li>
            <li><strong>Queue & liquidity.</strong> queue-claim tokenizes redemption requests; liquidity-facility advances against them.</li>
            <li><strong>Risk transfer.</strong> tranche-engine splits a pool into senior and junior books; npcs-treasury mints and redeems NPCS against the USDC sleeve.</li>
            <li><strong>Cash.</strong> payment-rails holds every USDC and asset escrow, settles DvP, and admits engines through an <code>EngineGrant</code>.</li>
          </ul>
        </Prose>
      </Band>
      <Band eyebrow="Trust flow" title="Who can touch the money">
        <Prose>
          <p>
            The operator that drives a live fill reports reveals and chooses the matching, but cannot redirect value. The seller is paid only into its own account, the buyer receives only into its own, and fees and reserve go only to accounts recorded at session open. For every fill <code>to_seller + fee + to_reserve == gross</code>, computed in u128 and checked against real balances.
          </p>
          <p>
            On devnet, the upgrade authority of payment-rails and credit-gate, and the NPCS mint authorities, sit with a Squads v4 2-of-2 multisig behind a 48-hour timelock; the engine programs are still upgraded by a single devnet deployer key. The live authority map is on the app&apos;s admin page. NUNDINA never holds the asset&apos;s freeze or permanent-delegate authority for issuer purposes, NAV publication, or custody of client assets.
          </p>
          <p>
            Details: <Link href="/docs/05-program-map-and-accounts">Program Map & Accounts</Link>, <Link href="/docs/12-deployment-governance-ops">Deployment, Governance & Ops</Link>.
          </p>
        </Prose>
      </Band>
    </>
  );
}
