import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement>;

/* Filled engraving motifs (hero) */

export function StarIcon(props: IconProps) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden {...props}>
      <path d="M12 2L15 8L21 9L17 14L18 20L12 17L6 20L7 14L3 9L9 8L12 2Z" opacity="0.9" />
    </svg>
  );
}

export function LaurelIcon(props: IconProps) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden {...props}>
      <path
        d="M12 2C11 5 8 7 6 8C9 10 11 12 11 15C10 17 8 19 6 20C10 20 14 17 15 14C16 11 15 7 12 2ZM18 8C16 7 13 5 12 2C9 7 8 11 9 14C10 17 14 20 18 20C16 19 14 17 13 15C13 12 15 10 18 8Z"
        opacity="0.9"
      />
    </svg>
  );
}

export function TempleIcon(props: IconProps) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden {...props}>
      <path d="M12 2L2 7V9H22V7L12 2ZM4 11H6V19H4V11ZM9 11H11V19H9V11ZM14 11H16V19H14V11ZM19 11H21V19H19V11ZM2 21V23H22V21H2Z" />
    </svg>
  );
}

export function CompassStarIcon(props: IconProps) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden {...props}>
      <path d="M12 0L14.2 9.8L24 12L14.2 14.2L12 24L9.8 14.2L0 12L9.8 9.8L12 0Z" />
    </svg>
  );
}

/* Hairline (stroked) icons — safeguards */

function Stroke({ d, ...props }: IconProps & { d: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" aria-hidden {...props}>
      <path d={d} strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} />
    </svg>
  );
}

export const ShieldCheckIcon = (p: IconProps) => (
  <Stroke
    d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z"
    {...p}
  />
);

export const BuildingIcon = (p: IconProps) => (
  <Stroke
    d="M19 21V5a2 2 0 00-2-2H7a2 2 0 00-2 2v16m14 0h2m-2 0h-5m-9 0H3m2 0h5M9 7h1m-1 4h1m4-4h1m-1 4h1m-5 10v-5a1 1 0 011-1h2a1 1 0 011 1v5m-4 0h4"
    {...p}
  />
);

export const LockIcon = (p: IconProps) => (
  <Stroke
    d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z"
    {...p}
  />
);

export const ClipboardIcon = (p: IconProps) => (
  <Stroke
    d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4"
    {...p}
  />
);

export const MenuIcon = (p: IconProps) => <Stroke d="M4 6h16M4 12h16m-7 6h7" {...p} />;

export const CloseIcon = (p: IconProps) => <Stroke d="M6 18L18 6M6 6l12 12" {...p} />;
