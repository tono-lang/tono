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

export const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

export function emptyForm(): MockForm {
  return { env: [], routes: [], passthrough: false };
}

/* mocks.json text -> form. Unparseable text yields null so the caller keeps
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

export interface RouteIssue {
  index: number;
  field: "path" | "status" | "body";
  message: string;
}

export function validate(form: MockForm): RouteIssue[] {
  const issues: RouteIssue[] = [];
  form.routes.forEach((route, index) => {
    if (!route.path.startsWith("/")) {
      issues.push({ index, field: "path", message: "path starts with /" });
    }
    const status = Number(route.status);
    if (!Number.isInteger(status) || status < 100 || status > 599) {
      issues.push({ index, field: "status", message: "status is 100-599" });
    }
    try {
      JSON.parse(route.body);
    } catch (err) {
      issues.push({ index, field: "body", message: `body is not JSON: ${String(err).replace(/^SyntaxError: /, "")}` });
    }
  });
  return issues;
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

export interface OpSuggestion {
  method: string;
  /* The concrete-ish path: template labels kept as {name} for the user to
     fill, which still reads better than starting from nothing. */
  path: string;
  /* The env vars the entry's fields read, worth seeding with $MOCK. */
  envKeys: string[];
}

/* Read the declared @http operations (and @env sources) out of the IR, so the
   form can offer one click per operation instead of hand-typed routes. */
export function suggestFromIr(irJson: string): OpSuggestion[] {
  interface Trait {
    id?: string;
    value?: { method?: string; path?: string };
  }
  interface Shape {
    traits?: Trait[];
    operations?: Shape[];
    fields?: { sources?: { env?: string }[] }[];
  }
  try {
    const ir = JSON.parse(irJson) as { modules?: { shapes?: Shape[] }[] };
    const out: OpSuggestion[] = [];
    const envKeys: string[] = [];
    const walkShape = (shape: Shape): void => {
      for (const field of shape.fields ?? []) {
        for (const source of field.sources ?? []) {
          if (typeof source.env === "string" && !envKeys.includes(source.env)) {
            envKeys.push(source.env);
          }
        }
      }
      const http = (shape.traits ?? []).find((t) => t.id === "http");
      if (http?.value?.method && http.value.path) {
        out.push({ method: http.value.method, path: http.value.path, envKeys });
      }
      for (const op of shape.operations ?? []) walkShape(op);
    };
    for (const module of ir.modules ?? []) {
      for (const shape of module.shapes ?? []) walkShape(shape);
    }
    /* All suggestions share the collected env keys. */
    return out.map((o) => ({ ...o, envKeys }));
  } catch {
    return [];
  }
}
