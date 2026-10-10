/**
 * The NUNDINA docs manifest. One chapter per markdown file in
 * `src/content/docs/`. The chapter list is the single source of truth for the
 * `/docs` sidebar, the docs index, and `generateStaticParams`.
 *
 * Content rules (frontend/info.md): devnet facts only, cited market data,
 * no APY/TVL/audit claims. Authors follow `src/content/docs/AUTHORING.md`.
 */

export type DocPart = "I" | "II" | "III" | "IV";

export type Chapter = {
  n: number;
  slug: string;
  title: string;
  short: string;
  blurb: string;
  part: DocPart;
};

export const PARTS: { id: DocPart; label: string }[] = [
  { id: "I", label: "Foundations" },
  { id: "II", label: "Architecture & Programs" },
  { id: "III", label: "Asset, Ops & Applications" },
  { id: "IV", label: "Legal, Risk & Roadmap" },
];

export const CHAPTERS: Chapter[] = [
  {
    n: 1,
    slug: "01-why-nundina",
    title: "Why NUNDINA",
    short: "Why",
    blurb: "The gate crisis, the marking problem, why now, and why Solana.",
    part: "I",
  },
  {
    n: 2,
    slug: "02-what-is-nundina",
    title: "What NUNDINA Is",
    short: "What",
    blurb: "Positioning, the primitives, the business model, and what it is not.",
    part: "I",
  },
  {
    n: 3,
    slug: "03-origins-and-evolution",
    title: "Origins & Evolution",
    short: "Origins",
    blurb: "The research corpus, the v1→v2 reframes, and the 2026 reality check.",
    part: "I",
  },
  {
    n: 4,
    slug: "04-high-level-architecture",
    title: "High-Level Architecture",
    short: "Architecture",
    blurb: "The layered view, the trust flow, and the two core mechanisms.",
    part: "II",
  },
  {
    n: 5,
    slug: "05-program-map-and-accounts",
    title: "Program Map & Accounts",
    short: "Programs",
    blurb: "The eight programs, the credit-math crate, PDAs, and transaction flows.",
    part: "II",
  },
  {
    n: 6,
    slug: "06-payment-rails",
    title: "payment-rails — The Cash Layer",
    short: "Cash layer",
    blurb: "Escrow, bonds, DvP, refunds, ROFR, advances, and the cash waterfall.",
    part: "II",
  },
  {
    n: 7,
    slug: "07-credit-gate",
    title: "credit-gate — Compliance at Activation",
    short: "Gate",
    blurb: "sRFC37, SAS attestations, the lot ledger, holder governor, block-list.",
    part: "II",
  },
  {
    n: 8,
    slug: "08-pricing-engines",
    title: "Pricing Engines — auction-engine & mark-engine",
    short: "Pricing",
    blurb: "Sealed-bid uniform-price sessions and making a stale appraisal usable.",
    part: "II",
  },
  {
    n: 9,
    slug: "09-queue-and-liquidity",
    title: "Queue & Liquidity — queue-claim & liquidity-facility",
    short: "Queue",
    blurb: "Tokenized redemption claims and the liquidation ladder.",
    part: "II",
  },
  {
    n: 10,
    slug: "10-tranching-treasury-math",
    title: "Tranching, Treasury & Math",
    short: "Tranching",
    blurb: "tranche-engine three-book accounting, npcs-treasury, and credit-math.",
    part: "II",
  },
  {
    n: 11,
    slug: "11-npcs-asset",
    title: "The NPCS Pilot Asset",
    short: "NPCS",
    blurb: "Token anatomy, the three invariants, movement topology, and lifecycle.",
    part: "III",
  },
  {
    n: 12,
    slug: "12-deployment-governance-ops",
    title: "Deployment, Governance & Ops",
    short: "Deploy & ops",
    blurb: "Squads governance, the authority map, registries, verification, runbooks.",
    part: "III",
  },
  {
    n: 13,
    slug: "13-off-chain-services",
    title: "Off-Chain Services",
    short: "Services",
    blurb: "Indexer, read API, NAV publisher, treasury agent, crank-runner, evidence.",
    part: "III",
  },
  {
    n: 14,
    slug: "14-applications",
    title: "Applications — dApp & Marketing Client",
    short: "Apps",
    blurb: "The chain-reading dApp and the public marketing site and this docs reader.",
    part: "III",
  },
  {
    n: 15,
    slug: "15-legal-risk-audit-roadmap",
    title: "Legal, Risk, Audit & Roadmap",
    short: "Legal",
    blurb: "Legal topology, counsel gates, audit scope, risk register, phases, status.",
    part: "IV",
  },
];

export function getChapter(slug: string): Chapter | undefined {
  return CHAPTERS.find((c) => c.slug === slug);
}

export function chaptersByPart(part: DocPart): Chapter[] {
  return CHAPTERS.filter((c) => c.part === part);
}

/** Adjacent chapters for prev/next links. */
export function neighbours(slug: string): { prev?: Chapter; next?: Chapter } {
  const i = CHAPTERS.findIndex((c) => c.slug === slug);
  if (i < 0) return {};
  return { prev: CHAPTERS[i - 1], next: CHAPTERS[i + 1] };
}

/** Deterministic heading slug used by both the reader and the on-page ToC. */
export function slugifyHeading(text: string): string {
  const s = text
    .toLowerCase()
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[`*_~]/g, "")
    .replace(/[^\w\s-]/g, "")
    .trim()
    .replace(/\s+/g, "-");
  return s || "section";
}

export type Heading = { level: number; text: string; id: string };

/**
 * Extract h2–h4 headings (in document order) for the on-page table of contents.
 * Skips fenced code blocks and h1 (the chapter title).
 */
export function extractHeadings(md: string): Heading[] {
  const out: Heading[] = [];
  const seen = new Map<string, number>();
  let inFence = false;
  for (const line of md.split("\n")) {
    if (/^\s*```/.test(line)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    const m = /^(#{2,4})\s+(.+?)\s*$/.exec(line);
    if (!m) continue;
    const level = m[1].length;
    const text = m[2]
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/[`*_~]/g, "")
      .trim();
    let id = slugifyHeading(text);
    const count = seen.get(id) ?? 0;
    seen.set(id, count + 1);
    if (count > 0) id = `${id}-${count + 1}`;
    out.push({ level, text, id });
  }
  return out;
}
