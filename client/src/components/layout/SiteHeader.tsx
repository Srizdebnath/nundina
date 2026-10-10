"use client";

import { useState } from "react";
import { usePathname } from "next/navigation";
import { Logo } from "@/components/ui/Logo";
import { CloseIcon, MenuIcon } from "@/components/ui/Icons";
import { SmartLink } from "@/components/ui/SmartLink";
import { HEADER_CTA, HEADER_NAV } from "@/lib/site";

export function SiteHeader() {
  const pathname = usePathname();
  const [open, setOpen] = useState(false);
  const isActive = (href: string) => pathname === href || pathname.startsWith(`${href}/`);

  return (
    <header className="fixed top-0 right-0 left-0 z-50 border-b border-gold-500/15 bg-obsidian-950/85 backdrop-blur-md transition-all duration-300">
      <div className="mx-auto flex h-20 max-w-7xl items-center justify-between px-6">
        <Logo />

        <nav
          aria-label="Main navigation"
          className="hidden items-center space-x-8 text-xs font-medium tracking-widest text-gold-500 uppercase lg:flex"
        >
          {HEADER_NAV.map((link) => (
            <SmartLink
              key={link.href}
              href={link.href}
              aria-current={isActive(link.href) ? "page" : undefined}
              className={`border-b pb-0.5 transition-all ${
                isActive(link.href)
                  ? "border-gold-400 text-gold-200"
                  : "border-transparent hover:border-gold-400 hover:text-gold-200"
              }`}
            >
              {link.label}
            </SmartLink>
          ))}
        </nav>

        <div className="flex items-center space-x-4">
          <SmartLink
            href={HEADER_CTA.href}
            className="hidden items-center rounded-sm border border-gold-400/40 px-4 py-2 text-xs font-semibold tracking-widest text-gold-200 uppercase transition-all duration-300 hover:border-gold-200 hover:bg-gold-100/10 hover:shadow-[0_0_15px_rgba(254,236,190,0.18)] sm:inline-flex"
          >
            <span>{HEADER_CTA.label}</span>
            <span className="ml-2 font-mono text-sm leading-none">→</span>
          </SmartLink>
          <button
            type="button"
            aria-label="Toggle navigation menu"
            aria-expanded={open}
            aria-controls="mobile-menu"
            onClick={() => setOpen((v) => !v)}
            className="p-2 text-gold-400 hover:text-gold-100 focus:outline-none lg:hidden"
          >
            {open ? <CloseIcon className="h-6 w-6" /> : <MenuIcon className="h-6 w-6" />}
          </button>
        </div>
      </div>

      {open && (
        <div
          id="mobile-menu"
          className="border-b border-gold-500/20 bg-obsidian-950/95 px-6 pt-2 pb-6 backdrop-blur-xl lg:hidden"
        >
          <div className="flex flex-col space-y-4 pt-2 text-xs font-medium tracking-widest text-gold-400 uppercase">
            {HEADER_NAV.map((link) => (
              <SmartLink
                key={link.href}
                href={link.href}
                onClick={() => setOpen(false)}
                className={`py-1 ${isActive(link.href) ? "text-gold-100" : "hover:text-gold-200"}`}
              >
                {link.label}
              </SmartLink>
            ))}
            <SmartLink
              href={HEADER_CTA.href}
              onClick={() => setOpen(false)}
              className="mt-2 inline-flex items-center justify-center rounded-sm bg-gold-300 px-4 py-2.5 text-xs font-semibold tracking-widest text-obsidian-950 uppercase"
            >
              {HEADER_CTA.label} →
            </SmartLink>
          </div>
        </div>
      )}
    </header>
  );
}
