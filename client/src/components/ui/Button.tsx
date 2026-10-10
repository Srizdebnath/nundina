import type { ReactNode } from "react";
import { SmartLink } from "./SmartLink";

type ButtonProps = {
  href: string;
  children: ReactNode;
  variant?: "gold" | "outline";
  arrow?: boolean;
  className?: string;
};

const base =
  "inline-flex w-full items-center whitespace-nowrap justify-center rounded-sm text-center font-cinzel text-xs uppercase tracking-widest2 transition-all duration-300 sm:w-auto";

const variants = {
  gold:
    "gold-btn-gradient px-8 py-3.5 font-bold text-obsidian-950 hover:-translate-y-0.5 active:translate-y-0",
  outline:
    "border border-gold-400/40 px-7 py-3.5 font-semibold text-gold-300 backdrop-blur-sm hover:border-gold-200 hover:bg-gold-100/5 hover:text-gold-100",
};

export function Button({ href, children, variant = "gold", arrow, className = "" }: ButtonProps) {
  return (
    <SmartLink href={href} className={`${base} ${variants[variant]} ${className}`}>
      <span>{children}</span>
      {arrow && <span className="ml-2 font-mono text-sm leading-none">→</span>}
    </SmartLink>
  );
}
