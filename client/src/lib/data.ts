/**
 * Every number on the landing page lives here, with its source.
 * Rule (frontend/info.md): nothing may claim what the repo or devnet cannot
 * verify. NUNDINA facts come from DEPLOYMENTS.md / STATUS.md (Solana devnet);
 * market facts are independent, cited, and labelled as such.
 */

export type Source = { label: string; href?: string; date?: string };

/* ── NUNDINA on devnet (info.md §8) ── */

export const PROGRAMS_DEVNET = 7;
export const PROGRAMS_TOTAL = 8;
export const PROGRAMS_MAINNET = 0;

export type TelemetryMetric = {
  label: string;
  value: string;
  note: string;
  emphasis?: boolean;
  dot?: boolean;
};

export const TELEMETRY: TelemetryMetric[] = [
  {
    label: "Programs on Devnet",
    value: `${PROGRAMS_DEVNET}/${PROGRAMS_TOTAL}`,
    note: `${PROGRAMS_MAINNET}/${PROGRAMS_TOTAL} on mainnet`,
  },
  {
    label: "NAV Publication #1",
    value: "1.000000",
    note: "USDC per NPCS · weekly",
    emphasis: true,
  },
  {
    label: "NPCS Supply",
    value: "1.000000",
    note: "Backed by 1 USDC",
  },
  {
    label: "Governance",
    value: "2-of-2",
    note: "Squads v4 · 48h timelock",
  },
  {
    label: "External Audit",
    value: "Pending",
    note: "Required before mainnet",
    dot: true,
  },
];

export const NPCS = {
  name: "NUNDINA Pilot Credit Sleeve",
  symbol: "NPCS",
  mint: "6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4",
  nav: "1.000000",
  supply: "1.000000",
  decimals: 6,
  holderCap: 8,
  seasoningDays: 7,
  explorer:
    "https://explorer.solana.com/address/6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4?cluster=devnet",
};

export const shortAddress = (addr: string) => `${addr.slice(0, 6)}…${addr.slice(-4)}`;

/* ── The 2026 gate crisis (info.md §7.2) — market data, NOT NUNDINA ── */

export type MarketCard = {
  tag: string;
  chip: string;
  title: string;
  blurb: string;
  stats: { label: string; value: string; strong?: boolean }[];
  bar: { label: string; value: string; pct: number };
  source: Source;
};

export const GATE_CRISIS: MarketCard[] = [
  {
    tag: "Market Data",
    chip: "Q3 2026",
    title: "Apollo Debt Solutions BDC",
    blurb: "Capped redemptions for a third straight quarter under its contractual 5% gate.",
    stats: [
      { label: "Requests (% NAV)", value: "14.7%", strong: true },
      { label: "Quarterly Cap", value: "5% of NAV" },
      { label: "Fund Size", value: "~$26B" },
      { label: "Outcome", value: "Capped" },
    ],
    // 5% cap ÷ 14.7% requested ≈ 34% of each request honored.
    bar: { label: "Requests honored (cap ÷ requests)", value: "≈ 34%", pct: 34 },
    source: {
      label: "Reuters",
      date: "2026-09-22",
      href: "https://www.reuters.com/business/apollo-private-credit-fund-redemption-requests-ease-third-quarter-2026-09-22/",
    },
  },
  {
    tag: "Market Data",
    chip: "Q1 2026",
    title: "BlackRock HPS Corporate Lending",
    blurb: "One of seven managers that capped withdrawals in Q1 2026 as requests ran past the gate.",
    stats: [
      { label: "Requests (% NAV)", value: "9.3%", strong: true },
      { label: "Quarterly Cap", value: "5% of NAV" },
      { label: "Fund Size", value: "$26B" },
      { label: "Outcome", value: "Capped" },
    ],
    // 5% cap ÷ 9.3% requested ≈ 54% of each request honored.
    bar: { label: "Requests honored (cap ÷ requests)", value: "≈ 54%", pct: 54 },
    source: {
      label: "CRS IN12674",
      date: "2026-04-02",
      href: "https://www.congress.gov/crs-product/IN12674",
    },
  },
];

/* ── Ecosystem strip (info.md §8.9) — real dependencies only ── */

export const ECOSYSTEM: { label: string; planned?: boolean }[] = [
  { label: "Solana" },
  { label: "Token-2022" },
  { label: "Token ACL sRFC-37" },
  { label: "SAS" },
  { label: "Squads" },
  { label: "Anchor" },
  { label: "LiteSVM" },
  { label: "Helius", planned: true },
];
