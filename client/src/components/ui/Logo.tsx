import Image from "next/image";
import Link from "next/link";
import logoMark from "@/assets/logo-mark.png";

/** Brand mark cut from public/logo.jpeg (transparent background). */
export function LogoMark({ className = "h-8 w-8" }: { className?: string }) {
  return <Image src={logoMark} alt="" aria-hidden className={`object-contain ${className}`} />;
}

export function Logo() {
  return (
    <Link href="/" aria-label="Nundina home" className="group flex items-center space-x-3">
      <LogoMark className="h-9 w-9 drop-shadow-[0_0_10px_rgba(240,205,141,0.35)] transition-transform duration-300 group-hover:scale-105" />
      <span className="font-cinzel text-lg font-bold tracking-widest2 text-gold-100 transition-colors group-hover:text-gold-300 md:text-xl">
        NUNDINA
      </span>
    </Link>
  );
}

export function FooterMark() {
  return (
    <div className="mb-4 flex items-center space-x-3">
      <LogoMark className="h-8 w-8" />
      <span className="font-cinzel text-lg font-bold tracking-widest2 text-gold-100">NUNDINA</span>
    </div>
  );
}
