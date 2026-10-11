"use client";
/** dApp tab bar: highlights the current section, scrolls horizontally on small screens. */
import Link from "next/link";
import { usePathname } from "next/navigation";

const TABS = [
  { href: "/app", label: "Overview" },
  { href: "/app/sessions", label: "Sessions" },
  { href: "/app/portfolio", label: "Portfolio" },
  { href: "/app/gate", label: "Gate check" },
  { href: "/app/queue", label: "Queue" },
  { href: "/app/facility", label: "Facility" },
  { href: "/app/tranches", label: "Tranches" },
  { href: "/app/treasury", label: "Treasury" },
  { href: "/app/data-room", label: "Data room" },
  { href: "/app/admin", label: "Admin" },
];

/** The tab bar before the pathname is known (dynamic routes stream it in). */
export function AppNavStatic() {
  return <Tabs path="" />;
}

export function AppNav() {
  return <Tabs path={usePathname()} />;
}

function Tabs({ path }: { path: string }) {
  const active = (href: string) => (href === "/app" ? path === "/app" : path === href || path.startsWith(`${href}/`));
  return (
    <nav className="app-tabs" aria-label="App sections">
      {TABS.map((t) => (
        <Link key={t.href} href={t.href} aria-current={active(t.href) ? "page" : undefined}>
          {t.label}
        </Link>
      ))}
    </nav>
  );
}
