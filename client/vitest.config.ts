import { fileURLToPath } from "node:url";
import { loadEnv } from "vite";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  // The deployment addresses live in .env (the same file every build reads).
  test: { include: ["test/**/*.test.ts"], environment: "node", env: loadEnv("production", process.cwd(), "NEXT_PUBLIC_") },
});
