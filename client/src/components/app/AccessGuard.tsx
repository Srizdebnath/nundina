/**
 * Server-side gate in front of page data (SRS 08 §7 Access). In allowlist
 * mode nothing below renders, and so nothing is read from the cluster for
 * the page, until the viewer has signed in; `always` gates a page in every
 * mode (the data room).
 */
import React from "react";
import { accessMode, viewer } from "@/lib/access";
import { logAccess, warnIfOpenInProduction } from "@/lib/access-log";
import { AccessGate } from "./AccessGate";
import { Banner } from "./ui";

export async function AccessGuard({
  children,
  always = false,
  what = "This pilot",
  path,
}: {
  children: React.ReactNode;
  always?: boolean;
  what?: string;
  /** Log every view and refusal of this page (data room, admin). */
  path?: string;
}) {
  warnIfOpenInProduction();
  if (!always && accessMode() === "open") return <>{children}</>;
  const me = await viewer();
  if (path) logAccess(me ? "gated-view" : "gated-refused", me, path);
  if (me) return <>{children}</>;
  return (
    <main className="wrap">
      <Banner />
      <AccessGate what={what} />
    </main>
  );
}
