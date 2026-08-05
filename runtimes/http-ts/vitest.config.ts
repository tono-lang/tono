import { defineConfig } from "vitest/config";

// Coverage is a real gate for this hand-written runtime: the report feeds both
// the local threshold below and the Sonar analysis (imported as lcov), so the
// runtime is measured, not excluded.
export default defineConfig({
  test: {
    coverage: {
      provider: "v8",
      reporter: ["text", "lcov"],
      reportsDirectory: "coverage",
      include: ["src/**/*.ts"],
      thresholds: {
        lines: 90,
        functions: 90,
        branches: 90,
        statements: 90,
      },
    },
  },
});
