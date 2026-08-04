/* One UI, two packagings. The hosted build is the capped demo: edit, preview
   the generated SDK, share by URL. The full build (what the CLI will serve
   locally, and what dev mode uses) also executes code against the SDK. The
   cap is a build-time capability, not a fork: everything else is shared. */

export interface Capabilities {
  run: boolean;
}

export function resolveCapabilities(mode: string | undefined): Capabilities {
  return { run: mode === "full" };
}

export const CAPABILITIES = resolveCapabilities(
  import.meta.env.VITE_PLAYGROUND_MODE ?? (import.meta.env.DEV ? "full" : "hosted"),
);
