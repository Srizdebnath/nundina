/**
 * Server-side gate in front of page data (SRS 08 §7 Access). In allowlist
 * mode nothing below renders, and so nothing is read from the cluster for
 * the page, until the viewer has signed in; `always` gates a page in every
 * mode (the data room).
 */
import React from "react";
import { accessMode, viewer } from "@/lib/access";
import { AccessGate } from "./AccessGate";
import { Banner } from "./ui";

export async function AccessGuard({ children, always = false, what = "This pilot" }: { children: React.ReactNode; always?: boolean; what?: string }) {
  if (!always && accessMode() === "open") return <>{children}</>;
  if (await viewer()) return <>{children}</>;
  return (
    <main className="wrap">
      <Banner />
      <AccessGate what={what} />
    </main>
  );
}
