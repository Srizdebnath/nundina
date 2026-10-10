import type { ComponentType, SVGProps } from "react";
import { SectionHeading } from "@/components/ui/SectionHeading";
import { BuildingIcon, ClipboardIcon, LockIcon, ShieldCheckIcon } from "@/components/ui/Icons";

const CARDS: { title: string; body: string; Icon: ComponentType<SVGProps<SVGSVGElement>> }[] = [
  {
    title: "Squads 2-of-2 + 48h",
    body: "Mint, delegate and upgrade authority sit behind a Squads v4 multisig with a 48-hour timelock. Devnet caveat: both keys were generated on one machine.",
    Icon: LockIcon,
  },
  {
    title: "Audited Infrastructure",
    body: "Built on dependencies with published audits — Squads v4, SPL Token-2022 and SAS. These audits cover the dependencies, not NUNDINA.",
    Icon: BuildingIcon,
  },
  {
    title: "On-Chain Proof",
    body: "NAV publications on-chain, program dump SHA-256 matched to recorded build hashes, and a keyless verify.sh anyone can run.",
    Icon: ShieldCheckIcon,
  },
  {
    title: "Audit Roadmap",
    body: "No third-party audit has been performed. An external audit is required before mainnet; the RFQ pack is ready but not yet sent.",
    Icon: ClipboardIcon,
  },
];

export function Safeguards() {
  return (
    <section id="security" className="relative border-t border-gold-500/20 bg-obsidian-950 py-24">
      <div className="mx-auto max-w-7xl px-6">
        <SectionHeading eyebrow="Honest Rigor" title="Security Posture, Stated Plainly">
          What is live, what is verifiable, and what is still pending — no borrowed audit badges.
        </SectionHeading>

        <div className="grid grid-cols-1 gap-6 md:grid-cols-2 lg:grid-cols-4">
          {CARDS.map(({ title, body, Icon }) => (
            <div key={title} className="etched-card group rounded-sm p-6 text-center">
              <div className="mx-auto mb-4 flex h-10 w-10 items-center justify-center text-gold-300 transition-colors group-hover:text-gold-100">
                <Icon className="h-8 w-8" />
              </div>
              <h4 className="mb-2 font-cinzel text-xs font-bold tracking-wider text-gold-100 uppercase">{title}</h4>
              <p className="text-[11px] leading-relaxed font-light text-gold-500">{body}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
