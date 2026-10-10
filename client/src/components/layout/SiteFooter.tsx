import { FooterMark } from "@/components/ui/Logo";
import { SmartLink } from "@/components/ui/SmartLink";
import { FOOTER_GROUPS, LEGAL_LINKS, PILOT_DISCLAIMER } from "@/lib/site";

export function SiteFooter() {
  return (
    <footer className="border-t border-gold-500/15 bg-obsidian-950 pt-16 pb-12 text-gold-500">
      <div className="mx-auto max-w-7xl px-6">
        <div className="grid grid-cols-2 gap-10 border-b border-gold-500/10 pb-12 md:grid-cols-5">
          <div className="col-span-2">
            <FooterMark />
            <p className="mb-6 max-w-sm text-xs leading-relaxed font-light text-gold-500">
              The pricing, queue &amp; risk-transfer engine for gated, NAV-priced private credit — on
              Solana. Not a venue. Not an AMM.
            </p>
            <div className="flex items-center space-x-2 font-mono text-xs text-gold-400">
              <span className="h-2 w-2 animate-pulse rounded-full bg-gold-300" />
              <span>Devnet pilot • Solana</span>
            </div>
          </div>

          {FOOTER_GROUPS.map((group) => (
            <div key={group.title}>
              <h5 className="mb-4 font-cinzel text-xs font-bold tracking-wider text-gold-200 uppercase">
                {group.title}
              </h5>
              <ul className="space-y-2.5 text-xs font-light">
                {group.links.map((link) => (
                  <li key={link.href}>
                    <SmartLink href={link.href} external={link.external} className="transition-colors hover:text-gold-200">
                      {link.label}
                      {link.external && <span className="ml-1 font-mono">↗</span>}
                    </SmartLink>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>

        <div className="mt-8 flex flex-col items-center justify-between gap-4 text-[11px] font-light text-gold-600 md:flex-row">
          <div className="text-center md:text-left">
            © 2026 NUNDINA. Solana devnet pilot — not audited, not offered publicly.{" "}
            <span className="font-mono text-gold-500">{PILOT_DISCLAIMER}</span>
          </div>
          <div className="flex space-x-6 font-mono">
            {LEGAL_LINKS.map((link) => (
              <SmartLink key={link.href} href={link.href} className="hover:text-gold-300">
                {link.label}
              </SmartLink>
            ))}
          </div>
        </div>
      </div>
    </footer>
  );
}
