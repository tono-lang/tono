/* Everything the mock editor derives from the IR so the user thinks in
   operations, not transport: the catalog of @http operations (name, route,
   env keys) and a valid sample response built from each operation's declared
   output type. The wire conventions live here too: i64/u64 travel as strings,
   an optional member is simply absent (two-state presence). */

type Tref =
  | { prim: string }
  | { ref: string; args?: Tref[] }
  | { list: Tref }
  | { map: [Tref, Tref] };

interface Member {
  name: string;
  required?: boolean;
  target: Tref;
  traits?: { id?: string; value?: unknown }[];
}

interface Shape {
  id: string;
  kind?: string;
  members?: Member[];
  values?: { name: string }[];
  discriminator?: string;
  fields?: { sources?: { env?: string }[] }[];
  operations?: Shape[];
  output?: { ref?: string; prim?: string };
  traits?: { id?: string; value?: { method?: string; path?: string } }[];
}

interface Ir {
  modules?: {
    shapes?: Shape[];
    operations?: Shape[];
    extensions?: { kind?: string; name?: string; bindings?: Record<string, string> }[];
  }[];
}

const PRIM_SAMPLES: Record<string, unknown> = {
  string: "hello",
  uuid: "0b8f8f2e-1e64-4c1c-9b6b-2f8d3a6a1c11",
  timestamp: "2024-01-01T12:00:00Z",
  date: "2024-01-01",
  duration: "5s",
  bytes: "aGVsbG8=",
  bool: true,
  float: 1.5,
  i64: "0",
  u64: "0",
};

function sampleOfTref(t: Tref, byId: Map<string, Shape>, depth: number): unknown {
  if (depth > 4) return null;
  if ("prim" in t) {
    if (t.prim in PRIM_SAMPLES) return PRIM_SAMPLES[t.prim];
    /* remaining prims are the narrow ints */
    return 0;
  }
  if ("list" in t) return [sampleOfTref(t.list, byId, depth + 1)];
  if ("map" in t) return {};
  if ("ref" in t) {
    const shape = byId.get(t.ref);
    if (!shape) return {};
    return sampleOfShape(shape, byId, depth + 1);
  }
  return null;
}

function sampleOfShape(shape: Shape, byId: Map<string, Shape>, depth: number): unknown {
  if (shape.kind === "enum") return shape.values?.[0]?.name ?? "unknown";
  if (shape.kind === "union") {
    const first = shape.members?.[0];
    if (!first) return {};
    const payload = sampleOfTref(first.target, byId, depth + 1);
    const tag = shape.discriminator ?? "kind";
    return typeof payload === "object" && payload !== null
      ? { [tag]: first.name, ...payload }
      : { [tag]: first.name };
  }
  const out: Record<string, unknown> = {};
  for (const member of shape.members ?? []) {
    /* Optional members stay absent: presence is the two-state signal. */
    if (member.required === false) continue;
    out[member.name] = sampleOfTref(member.target, byId, depth + 1);
  }
  return out;
}

export interface OpInfo {
  /* Local op name (get_user), the user-facing identity of the mock. */
  name: string;
  /* The HTTP route, absent for an operation implemented by bespoke code. */
  route: { method: string; path: string } | null;
  /* Languages an ext impl binds, for the bespoke badge. */
  implLangs: string[];
  /* A valid response body for the declared output, pretty-printed. */
  sampleBody: string;
  envKeys: string[];
}

export function opCatalog(irJson: string): OpInfo[] {
  try {
    const ir = JSON.parse(irJson) as Ir;
    const byId = new Map<string, Shape>();
    const ops: Shape[] = [];
    const envKeys: string[] = [];
    for (const module of ir.modules ?? []) {
      for (const shape of module.shapes ?? []) {
        byId.set(shape.id, shape);
        for (const field of shape.fields ?? []) {
          for (const source of field.sources ?? []) {
            if (typeof source.env === "string" && !envKeys.includes(source.env)) {
              envKeys.push(source.env);
            }
          }
        }
        for (const op of shape.operations ?? []) ops.push(op);
      }
      for (const op of module.operations ?? []) ops.push(op);
    }
    const impls = new Map<string, string[]>();
    for (const module of ir.modules ?? []) {
      for (const ext of module.extensions ?? []) {
        if (ext.kind === "impl" && ext.name) {
          impls.set(ext.name, Object.keys(ext.bindings ?? {}));
        }
      }
    }
    const out: OpInfo[] = [];
    for (const op of ops) {
      const http = (op.traits ?? []).find((t) => t.id === "http");
      let body: unknown = {};
      if (op.output?.ref) {
        const shape = byId.get(op.output.ref);
        if (shape) body = sampleOfShape(shape, byId, 0);
      } else if (op.output?.prim) {
        body = sampleOfTref({ prim: op.output.prim }, byId, 0);
      }
      const dotted = op.id.split("#")[1] ?? op.id;
      out.push({
        name: dotted.split(".").pop() ?? dotted,
        route:
          http?.value?.method && http.value.path
            ? { method: http.value.method, path: http.value.path }
            : null,
        implLangs: impls.get(dotted) ?? [],
        sampleBody: JSON.stringify(body, null, 2),
        envKeys,
      });
    }
    return out;
  } catch {
    return [];
  }
}

/* Whether a declared path template matches a concrete request path: same
   segment count, and a {label} segment matches any non-empty segment. */
export function routeMatches(template: string, path: string): boolean {
  const t = template.split("/");
  const p = path.split("/");
  if (t.length !== p.length) return false;
  return t.every((seg, i) => (/^\{[^}]+\}$/.test(seg) ? p[i] !== "" : seg === p[i]));
}
