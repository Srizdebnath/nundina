/**
 * Site-wide routes and links. Route map follows frontend/info.md §3:
 * marketing pages live at the root, the live chain-reading dApp (Nundina/app)
 * is served under `/app` (or wherever NEXT_PUBLIC_APP_URL points).
 */

export const APP_URL = process.env.NEXT_PUBLIC_APP_URL ?? "/app";
export const REPO_URL = "https://github.com/Srizdebnath/nundina";
export { RPC_URL as DEVNET_RPC_URL } from "@/lib/programs";

export const PILOT_DISCLAIMER = "PILOT-DEMO-ASSET — NOT AN OFFER";

export type NavLink = { label: string; href: string; external?: boolean };

export const HEADER_NAV: NavLink[] = [
  { label: "Protocol", href: "/protocol" },
  { label: "Architecture", href: "/architecture" },
  { label: "Security", href: "/security" },
  { label: "Docs", href: "/docs" },
  { label: "Deployments", href: "/deployments" },
];

export const HEADER_CTA: NavLink = { label: "Enter Nundina", href: APP_URL };

export const FOOTER_GROUPS: { title: string; links: NavLink[] }[] = [
  {
    title: "Protocol",
    links: [
      { label: "Protocol", href: "/protocol" },
      { label: "Architecture", href: "/architecture" },
      { label: "Security", href: "/security" },
      { label: "Deployments", href: "/deployments" },
      { label: "Roadmap & Status", href: "/status" },
    ],
  },
  {
    title: "App",
    links: [
      { label: "Sessions", href: `${APP_URL}/sessions` },
      { label: "Portfolio", href: `${APP_URL}/portfolio` },
      { label: "Gate Check", href: `${APP_URL}/gate` },
      { label: "Data Room", href: `${APP_URL}/data-room` },
      { label: "Admin", href: `${APP_URL}/admin` },
    ],
  },
  {
    title: "Docs",
    links: [
      { label: "Overview", href: "/docs" },
      { label: "Why NUNDINA", href: "/docs/01-why-nundina" },
      { label: "Architecture", href: "/docs/04-high-level-architecture" },
      { label: "NPCS Asset", href: "/docs/11-npcs-asset" },
      { label: "Audit & Legal", href: "/docs/15-legal-risk-audit-roadmap" },
      { label: "GitHub", href: REPO_URL, external: true },
    ],
  },
];

export const LEGAL_LINKS: NavLink[] = [
  { label: "Terms", href: "/legal/terms" },
  { label: "Privacy", href: "/legal/privacy" },
  { label: "SAS Attestations", href: `${APP_URL}/gate` },
];
