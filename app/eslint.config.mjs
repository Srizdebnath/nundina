// ESLint flat config (#100: P0 exit criterion "app lint"). Next 16 removed
// `next lint`; the same rules run through the ESLint CLI (`npm run lint`).
import { defineConfig, globalIgnores } from "eslint/config";
import nextVitals from "eslint-config-next/core-web-vitals";
import nextTs from "eslint-config-next/typescript";

export default defineConfig([
  ...nextVitals,
  ...nextTs,
  globalIgnores([".next/**", "out/**", "build/**", "next-env.d.ts"]),
]);
