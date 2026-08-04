/* The structured model behind the mock editor: env rows, route cards, and the
   passthrough switch, converting to and from the mocks.json text that runs and
   share links speak. Field-level validation lives here so the form can point
   at the exact field instead of failing the whole JSON. */
import type { RunConfig } from "./run";

export interface RouteRow {
  method: string;
  path: string;
  status: string;
  /* JSON text; validated per row. */
  body: string;
}

export interface EnvRow {
  key: string;
  value: string;
}

export interface MockForm {
  env: EnvRow[];
  routes: RouteRow[];
  passthrough: boolean;
}

export function emptyForm(): MockForm {
  return { env: [], routes: [], passthrough: false };
}

/* mocks.json text -> form. Unparsable text yields null so the caller keeps
   the raw editor open instead of dropping user content. */
export function formFromJson(text: string): MockForm | null {
  try {
    const raw = JSON.parse(text) as {
      env?: Record<string, string>;
      routes?: Record<string, { status?: number; body?: unknown }>;
      passthrough?: boolean;
    };
    const env = Object.entries(raw.env ?? {}).map(([key, value]) => ({
      key,
      value: String(value),
    }));
    const routes = Object.entries(raw.routes ?? {}).map(([route, spec]) => {
      const space = route.indexOf(" ");
      return {
        method: space === -1 ? "GET" : route.slice(0, space).toUpperCase(),
        path: space === -1 ? route : route.slice(space + 1),
        status: String(spec.status ?? 200),
        body: spec.body === undefined ? "{}" : JSON.stringify(spec.body, null, 2),
      };
    });
    return { env, routes, passthrough: raw.passthrough ?? false };
  } catch {
    return null;
  }
}

/* form -> mocks.json text. Callers validate first; an invalid row falls back
   to an empty body rather than emitting broken JSON. */
export function formToJson(form: MockForm): string {
  const env: Record<string, string> = {};
  for (const row of form.env) {
    if (row.key.trim() !== "") env[row.key.trim()] = row.value;
  }
  const routes: Record<string, { status: number; body: unknown }> = {};
  for (const route of form.routes) {
    if (route.path.trim() === "") continue;
    let body: unknown;
    try {
      body = JSON.parse(route.body);
    } catch {
      body = {};
    }
    routes[`${route.method} ${route.path.trim()}`] = {
      status: Number(route.status) || 200,
      body,
    };
  }
  const out: Partial<RunConfig> = { env, routes };
  if (form.passthrough) out.passthrough = true;
  return JSON.stringify(out, null, 2);
}
