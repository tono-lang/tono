/// <reference types="vitest/config" />
import { defineConfig } from "vite";

// base "./" keeps the bundle relocatable so the same build works on GitHub
// Pages project sites (served under /playground/) and any other static host.
export default defineConfig({
  base: "./",
  build: {
    target: "es2022",
  },
  test: {
    // The staged compiler source under compiler/vendor ships its own test
    // suites; only this repo's tests belong to this runner.
    include: ["tests/**/*.test.ts"],
  },
});
