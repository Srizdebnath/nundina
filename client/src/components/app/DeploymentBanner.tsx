/**
 * Shown on every dApp page while a `NEXT_PUBLIC_*` address variable is missing
 * or malformed. Names the variables to set, so a misconfigured deployment is
 * obvious instead of silently reading the wrong chain accounts.
 */
import React from "react";
import { MISSING_ENV } from "@/lib/addresses";

export function DeploymentBanner() {
  if (MISSING_ENV.length === 0) return null;
  return (
    <div className="wrap" style={{ paddingBottom: 0 }}>
      <div className="panel" style={{ borderLeft: "3px solid var(--warn)" }}>
        <p className="small" style={{ margin: 0 }}>
          <strong>Deployment not configured.</strong> Set {MISSING_ENV.length === 1 ? "this variable" : "these variables"} in
          the environment (see <code>client/.env.example</code>): <code>{MISSING_ENV.join(", ")}</code>. Reads for the
          affected programs come back empty until then.
        </p>
      </div>
    </div>
  );
}
