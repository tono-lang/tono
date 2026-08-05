import { describe, expect, it } from "vitest";

import * as runtime from "../src/index";
import { execute } from "../src/execute";
import type { WireDescriptor } from "../src/descriptor";

// The runtime ships zero auth (ADR-0028): auth is bespoke via the extension
// model, and a header is only ever set through ClientOptions.headers.
describe("no built-in auth", () => {
  it("exports only the transport surface at runtime, nothing auth-shaped", () => {
    const names = Object.keys(runtime).sort();
    expect(names).toEqual(["assertExclusiveTransport", "execute"]);
    for (const name of names) {
      const lower = name.toLowerCase();
      expect(lower).not.toContain("auth");
      expect(lower).not.toContain("sign");
      expect(lower).not.toContain("token");
      expect(lower).not.toContain("credential");
    }
  });

  it("sets an Authorization header only when the caller supplies it, never by itself", async () => {
    const seen: Record<string, string>[] = [];
    const fetchImpl = (async (_url: string, init?: RequestInit) => {
      seen.push((init?.headers as Record<string, string>) ?? {});
      return new Response("{}", { status: 200 });
    }) as unknown as typeof fetch;

    const descriptor: WireDescriptor = {
      http_method: "GET",
      uri: "/x",
      bindings: [],
      response_bindings: [],
      success: [[200, null]],
    };

    await execute(descriptor, {}, { baseUrl: "https://api.test", fetch: fetchImpl });
    expect(seen[0].Authorization).toBeUndefined();

    await execute(descriptor, {}, {
      baseUrl: "https://api.test",
      fetch: fetchImpl,
      headers: { Authorization: "Bearer x" },
    });
    expect(seen[1].Authorization).toBe("Bearer x");
  });
});
