// A bespoke bearer-auth recipe: the client_init hook the generated SDK binds
// to. Construction resolves the entry's declared fields (API_TOKEN from the
// environment) into the Settings; this hook runs on top (bespoke wins) and
// writes the Authorization header into the transport state, so every request
// carries it with no per-call plumbing and no environment read of its own.
//
// This is a recipe to copy, not a shipped scheme: there is no built-in auth.
//
// The generated serde file imports this hook relative to itself, so drop the
// ext/ directory next to the generated SDK files.

import type { Settings } from "../../auth";

export function applyBearer(settings: Settings): void {
  if (settings.apiToken === "") {
    throw new Error("API_TOKEN is not set");
  }
  settings.headers["authorization"] = `Bearer ${settings.apiToken}`;
}
