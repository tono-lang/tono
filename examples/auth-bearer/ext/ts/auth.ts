// A bespoke bearer-auth recipe: the before_request hook the generated SDK binds
// to. The runtime hands it the CanonicalRequest before sending; it returns a
// copy with the Authorization header set. The SDK wraps this call at the
// boundary, so a throw here surfaces as a ContractError.
//
// This is a recipe to copy, not a shipped scheme: there is no built-in auth.
// Swap the token source for your own (env var, config, a refresh call once the
// async hook slot lands).

import type { CanonicalRequest } from "@tono/http-runtime-ts";

// The token source. A real integration reads it from configuration or a secret
// store rather than a module constant.
function token(): string {
  return process.env.API_TOKEN ?? "";
}

export function addBearer(req: CanonicalRequest): CanonicalRequest {
  return {
    ...req,
    headers: { ...req.headers, authorization: `Bearer ${token()}` },
  };
}
