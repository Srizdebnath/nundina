import type { Metadata } from "next";
import Link from "next/link";
import { Band, Cards, PageHero, Prose } from "@/components/page/PageHero";
import { Button } from "@/components/ui/Button";
import { APP_URL } from "@/lib/site";

export const metadata: Metadata = {
  title: "Protocol",
  description: "The pricing, queue and risk-transfer primitives NUNDINA builds for gated, NAV-priced private credit.",
};

const PRIMITIVES = [
  {
    meta: "mark-engine",
    title: "Marking",
    body: "The issuer publishes NAV on-chain on a weekly cadence. The mark ages from Fresh to Warned to Stale, and every engine that prices against it checks that state first.",
  },
  {
    meta: "auction-engine",
    title: "Pricing",
    body: "Sealed-bid batch sessions: commit, reveal, one uniform clearing price, pro-rata fills. No passive LP ever trades against a stale NAV.",
  },
  {
    meta: "queue-claim",
    title: "Queue claims",
    body: "A pro-rated redemption request becomes a dated, attested claim that can be sold or pledged instead of waiting in the queue.",
  },
  {
    meta: "liquidity-facility",
    title: "Liquidity facility",
    body: "Advances against queue claims and NPCS, with health checks, a stale-mark guard and a liquidation ladder.",
  },
  {
    meta: "tranche-engine",
    title: "Tranching",
    body: "Senior and junior pools over the same sleeve, with a coupon waterfall and loss allocation that depend on the mark.",
  },
  {
    meta: "credit-gate · payment-rails",
    title: "Compliance & cash",
    body: "Compliance at account activation (Token ACL + SAS) rather than in a transfer hook. Every fill settles DvP in USDC through the rails, in the same instruction.",
  },
];

export default function ProtocolPage() {
  return (
    <>
      <PageHero eyebrow="Protocol" title="The Mechanism, Not the Venue">
        NUNDINA prices a quarterly-marked, gated credit position, and turns a denied redemption request into a claim that can be priced and financed. It is not an issuer, a lending market, a DEX or a credential registry.
      </PageHero>
      <Band eyebrow="Four positions" title="What the design commits to">
        <Prose>
          <ul>
            <li><strong>Be the mechanism, not the venue.</strong> The venue layer is commoditizing; the pricing, queue and risk-transfer layer is empty.</li>
            <li><strong>Never put a passive LP in front of a stale NAV.</strong> Batch auction or nothing.</li>
            <li><strong>Compliance at account activation, not at transfer.</strong> Token ACL + SAS, not transfer hooks.</li>
            <li><strong>The queue is the asset.</strong> A pro-rated redemption request is a dated claim that can be priced and financed.</li>
          </ul>
        </Prose>
      </Band>
      <Band eyebrow="Primitives" title="What NUNDINA builds">
        <Cards items={PRIMITIVES} />
      </Band>
      <Band eyebrow="Lifecycle" title="A pilot deal, end to end">
        <Prose>
          <ol className="[&>li]:list-decimal">
            <li>A seller escrows NPCS and a USDC bond with payment-rails; buyers escrow cash at quantity × limit.</li>
            <li>auction-engine takes the session&apos;s reference mark from mark-engine at open, collects sealed commitments, then reveals.</li>
            <li>The book clears at one price; credit-gate&apos;s <code>settle_check</code> runs inside the value-moving transaction.</li>
            <li>Each fill pays the seller USDC and delivers NPCS to the buyer in the same instruction, then a print is published.</li>
            <li>Holders who need to exit outside a session submit a redemption request; the attested request becomes a queue claim.</li>
          </ol>
          <p>
            Read the full mechanics in <Link href="/docs/02-what-is-nundina">What NUNDINA Is</Link> and{" "}
            <Link href="/docs/04-high-level-architecture">High-Level Architecture</Link>.
          </p>
        </Prose>
        <div className="mt-10 flex flex-col gap-4 sm:flex-row">
          <Button href={APP_URL} arrow>Open the app</Button>
          <Button href="/docs" variant="outline">Read the docs</Button>
        </div>
      </Band>
    </>
  );
}
