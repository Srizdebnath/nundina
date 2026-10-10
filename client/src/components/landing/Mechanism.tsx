import { SectionHeading } from "@/components/ui/SectionHeading";

const STEPS = [
  {
    numeral: "I",
    title: "Attest & Validate",
    body: "Holders carry SAS identity and accreditation attestations; credit-gate validates them and returns a typed decision.",
  },
  {
    numeral: "II",
    title: "Permissionless Thaw",
    body: "Token ACL (sRFC-37) thaws the frozen-by-default account only for admitted holders — subject to an 8-holder cap and 7-day seasoning.",
  },
  {
    numeral: "III",
    title: "NAV Deposit & Auction",
    body: "Mint 1:1 at NAV, or trade in a sealed-bid uniform-price batch auction: commit, reveal, uniform clear, pro-rata, issuer ROFR.",
  },
  {
    numeral: "IV",
    title: "Claim & DvP Settle",
    body: "A gated redemption becomes a queue-claim to sell or pledge; payment-rails settles delivery-versus-payment in USDC and prints.",
  },
];

export function Mechanism() {
  return (
    <section id="architecture" className="relative overflow-hidden bg-obsidian-900 py-28">
      <div className="mx-auto max-w-7xl px-6">
        <SectionHeading eyebrow="Execution Mechanics" title="From Attestation to Settlement" className="mb-20 max-w-2xl">
          What actually runs on devnet today. Tranche selection is roadmap.
        </SectionHeading>

        <div className="relative grid grid-cols-1 gap-8 md:grid-cols-4">
          <div className="hairline-gold absolute top-7 right-12 left-12 z-0 hidden h-px md:block" />
          {STEPS.map((s) => (
            <div key={s.numeral} className="relative z-10 flex flex-col items-center text-center">
              <div className="mb-6 flex h-14 w-14 items-center justify-center rounded-full border border-gold-400 bg-obsidian-950 font-cinzel text-lg font-bold text-gold-200 shadow-[0_0_15px_rgba(254,236,190,0.15)]">
                {s.numeral}
              </div>
              <h4 className="mb-2 font-cinzel text-sm font-bold tracking-wider text-gold-100 uppercase">{s.title}</h4>
              <p className="text-xs leading-relaxed font-light text-gold-400">{s.body}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
