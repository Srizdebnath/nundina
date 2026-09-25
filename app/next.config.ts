import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Pilot UI is desktop-first, private-allowlist (SRS 08 §7). No i18n, no
  // experimental flags — add only with reason.
};

export default nextConfig;
