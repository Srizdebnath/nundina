/**
 * Shown on every page while `lib/addresses.ts` still holds `__PLACEHOLDER__`
 * tokens. The app is fully functional underneath — it resolves each id from the
 * program IDL and reads the real chain — so this is a status line, not a
 * blocker, and it disappears the moment the manifest is swapped in.
 */
import React from "react";
import { DEPLOYMENT_UNRESOLVED, UNRESOLVED_KEYS } from "@/lib/addresses";

export function DeploymentBanner() {
  if (!DEPLOYMENT_UNRESOLVED) return null;
  return (
    <div className="wrap">
      <div className="panel" style={{ borderLeft: "3px solid var(--warn, #b8860b)" }}>
        <p className="small">
          <strong>Awaiting deployment.</strong> {UNRESOLVED_KEYS.length} address
          {UNRESOLVED_KEYS.length === 1 ? "" : "es"} in <code>lib/addresses.ts</code> still{" "}
          {UNRESOLVED_KEYS.length === 1 ? "is a" : "are"} placeholder token
          {UNRESOLVED_KEYS.length === 1 ? "" : "s"}. Pages resolve them from the program IDL and
          read the chain for real, so anything shown below is live — but a redeploy needs{" "}
          <code>{UNRESOLVED_KEYS.join(", ")}</code> filled in.
        </p>
      </div>
    </div>
  );
}
