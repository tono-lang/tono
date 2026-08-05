import { defaultExclude, defineConfig } from "vitest/config";

// Coverage is a real gate for this hand-written runtime: the report feeds both
// the local threshold below and the Sonar analysis (imported as lcov), so the
// runtime is measured, not excluded.
export default defineConfig({
  test: {
    // parity.test.ts imports a generated SDK's own modules (./parity/client,
    // ./http): they only resolve once scripts/run-parity.sh has copied this
    // file next to a freshly generated SDK and run it from there. Run in
    // place against this source tree, those imports do not resolve.
    //
    // This package itself is retired once Go and Rust are repointed the same
    // way TypeScript is here: when that happens, parity.test.ts must move out
    // (to wherever the parity harnesses end up living once none of them has a
    // runtime package to call home) before this directory is deleted, or the
    // parity gate goes with it silently, with nothing left to notice.
    exclude: [...defaultExclude, "test/parity.test.ts"],
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
