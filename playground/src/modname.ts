/* The module name the playground compiles under: it becomes the IR module id,
   the Go package, the generated paths. Canonical names are snake_case, so the
   input is folded to that shape instead of rejected. */
export const DEFAULT_MODULE = "playground";

export function sanitizeModuleName(raw: string): string {
  const folded = raw
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, "_")
    .replace(/^[0-9_]+/, "")
    .replace(/_+$/, "");
  return folded === "" ? DEFAULT_MODULE : folded;
}
