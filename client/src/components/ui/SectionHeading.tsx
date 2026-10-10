import type { ReactNode } from "react";

type SectionHeadingProps = {
  eyebrow: string;
  title: ReactNode;
  children?: ReactNode;
  className?: string;
  /** Large titles go up to 6xl (paradigm); compact ones cap at 5xl. */
  size?: "large" | "compact";
};

/** Centered Cinzel eyebrow + Cormorant title + light lede, as used by most sections. */
export function SectionHeading({ eyebrow, title, children, className = "max-w-3xl mb-16", size = "compact" }: SectionHeadingProps) {
  return (
    <div className={`mx-auto text-center ${className}`}>
      <span className="mb-3 block font-cinzel text-xs font-semibold uppercase tracking-widest3 text-gold-400">
        {eyebrow}
      </span>
      <h2 className={`font-cormorant text-4xl leading-tight font-normal text-gold-100 sm:text-5xl ${size === "large" ? "lg:text-6xl" : ""}`}>
        {title}
      </h2>
      {children && (
        <p className="mt-5 text-sm leading-relaxed font-light text-gold-400 sm:text-base">{children}</p>
      )}
    </div>
  );
}
