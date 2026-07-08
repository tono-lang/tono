# IR JSON encoding

The serialized IR is the wire contract between the OCaml frontend (the source of
truth) and the Rust backend (the mirror). This document defines the encoding so
both sides reference one source. The golden fixtures under `fixtures/` are the
arbiter: they are generated from the frontend encoder and decoded by the backend,
and any divergence breaks the build.

## Version envelope

The top-level document is:

```json
{ "tono_ir_version": 1, "modules": [ /* module objects */ ] }
```

`tono_ir_version` is a single monotonic integer, not a semantic version. It is
bumped by one on every incompatible change to this encoding. A decoder that sees
a version it does not recognize fails loudly rather than attempting a partial
decode; there is no negotiation or multi-version support.

The current version is **1**.

## Modules

```json
{ "name": "payments.common", "shapes": [ /* shapes */ ], "operations": [ /* shapes */ ] }
```

A module's `name` is its qualified path: one `.tono` file is one module, named
by its path from the project root with the extension dropped and the separators
turned into dots (`payments/charge.tono` -> `payments.charge`). Every shape id and
every nominal reference is namespaced as `module#local` (see
[Module identity](#module-identity-imports-and-visibility)).

## Primitives

A primitive is a bare JSON string, one of:

```
"bool" "string" "bytes" "float" "timestamp" "date" "duration" "uuid"
"i8" "i16" "i32" "i64" "u8" "u16" "u32" "u64"
```

Integer widths are closed to `{8, 16, 32, 64}`. There is no `decimal`. Any other
string fails to decode.

## Type references

A type reference is a single-key tagged object, except `ref`, which carries a
sibling `args` array. Generic application is data: there is no synthesized
wrapper shape.

```json
{ "prim": "i32" }
{ "ref": "payments#Charge", "args": [] }
{ "ref": "core#Page", "args": [ { "ref": "payments#Charge", "args": [] } ] }
{ "param": "T" }
{ "list": { "prim": "string" } }
{ "map": [ { "prim": "string" }, { "prim": "string" } ] }
```

`args` is `[]` for a non-generic application and is always present. Decoding
rejects an object with zero recognized variant keys, more than one recognized
variant key, or an unexpected sibling key.

## Members

```json
{
  "name": "amount",
  "target": { "prim": "u64" },
  "required": true,
  "default": 0,
  "constraints": [ /* core constraints */ ],
  "traits": [ /* traits */ ]
}
```

- `required: false` denotes a nullable `T?`; `required: true` denotes `T`. There
  is no third state. `null` and absent collapse to the same value.
- `default` is the raw JSON value the client fills in and always sends. The key
  is omitted when there is no default, and present (possibly `null`) otherwise.
  A default is independent of `required`.
- `constraints` and `traits` are always present arrays (possibly empty).

## Constraints (core vocabulary)

A core constraint is a single-key tagged object. Sub-fields that are absent are
omitted.

```json
{ "range": { "min": 0.0, "max": 100.0, "exclMin": true, "exclMax": false } }
{ "length": { "min": 1, "max": 255 } }
{ "pattern": "^[a-z]+$" }
{ "multipleOf": 0.25 }
```

`range` always carries the boolean `exclMin`/`exclMax`; `min`/`max` are omitted
when absent. `length` omits `min`/`max` when absent. Floats are finite (no
NaN/Inf). Custom and any non-core constraint live in the trait bag, never here.

## Traits

A trait carries an id and an arbitrary JSON value:

```json
{ "id": "core#wire", "value": "bank_account" }
```

The value round-trips unchanged, including objects, arrays, and `null`. Integers
keep full precision within the signed/unsigned 64-bit range (`i64`/`u64`); the
IR's own integer types never exceed this, and a value beyond it is outside the
contract.

## Shapes

A shape is internally tagged by a `kind` field, flattened next to `id` and
`traits`. There are exactly five kinds.

```json
{ "id": "payments#Charge", "kind": "structure",
  "params": [], "members": [ /* members */ ], "traits": [] }

{ "id": "payments#Source", "kind": "union",
  "params": [], "members": [ /* members */ ], "discriminator": "type", "traits": [] }

{ "id": "payments#Status", "kind": "enum",
  "backing": "string", "values": [ ["active", null], ["closed", null] ],
  "open": true, "traits": [] }

{ "id": "payments#Payments", "kind": "service",
  "operations": [ "payments#ListCharges" ], "traits": [] }

{ "id": "payments#ListCharges", "kind": "operation",
  "input": null, "output": { "ref": "core#Page", "args": [ /* ... */ ] },
  "errors": [], "traits": [] }
```

- `union` always emits an explicit `discriminator` (default `"type"`).
- `enum` carries `backing` (`"string"` or `"int"`), `values` as `[name, intOrNull]`
  pairs, and the `open` flag. The implicit unknown variant of an open enum is a
  decode-time concern of the backend and is not materialized here.
- `operation` carries `input`/`output` as a type reference or `null`, and
  `errors` as an array of type references. They are type references (not bare
  ids) so an operation can return an applied generic directly.

## Numbers

- **Floats.** Both sides emit finite IEEE-754 doubles, but their *text* can
  differ (one may print `1e-05` where the other prints `0.00001`). The two are
  the same value, so cross-language agreement is checked by comparing the parsed
  JSON *data* (numbers by value, object keys order-independent), not the raw
  bytes. NaN and infinity are not valid JSON and are rejected on both encode and
  decode.
- **Integers in `default`/trait values.** Arbitrary JSON, preserved exactly
  within the signed/unsigned 64-bit range. Values outside `i64`/`u64` are
  outside the contract.
- **Structured integer fields** (`length` bounds, enum values) carry small
  counts in practice; they round-trip exactly within a signed 63-bit integer.
- **Meta-schema vs runtime wire.** Integer `default`/trait values are plain JSON
  numbers *in the IR*. How a generated SDK serializes an `i64` on its own runtime
  wire (e.g. as a string) is a separate concern and does not affect this encoding.

## Module identity, imports, and visibility

- **Identity by path.** A module is one `.tono` file; its qualified name is the
  file path from the project root, dotted (`payments/charge.tono` ->
  `payments.charge`). There is no `package` declaration: the path is the identity.
- **Qualified ids.** Every top-level shape id and every nominal `ref` is
  `module#local`, where `module` is the dotted module name and `local` is the
  snake_case declared name (`payments.common#money`). A reference to a type in the
  same module carries that module's own prefix; the backend drops the prefix when
  it equals the file's module, so no self-import is generated.
- **Imports are a resolution concept, not IR.** Between modules, a `.tono` file
  imports another (`import payments.common [as c]`) and references its types
  qualified (`common.money`). Imports steer name resolution in the frontend and
  leave no node in the IR: by the time a reference reaches the wire it is already
  a fully-qualified `module#local` id. The module import graph must be a DAG;
  a cycle is a compile error.
- **Visibility.** `pub` on a top-level declaration exports it (it becomes visible
  to other modules and is emitted in the SDK) and rides the IR as a `pub` trait
  on the shape. A private (non-`pub`) shape is visible only within its own module;
  referencing it across a module boundary is a compile error. Within a module
  everything is visible without `pub` or an import.

## Generated package mapping

The backend maps each module to an idiomatic sub-package per target, derived from
the dotted name:

| Module | Rust | Go | TypeScript |
|---|---|---|---|
| `payments.common` | `crate::payments::common` (`rust/payments/common.rs`) | package `common` (`go/payments/common/common.go`) | `./payments/common` (`typescript/payments/common.ts`) |

A single-segment module keeps the flat layout (`rust/payments.rs`). Two config
hooks, consumed by codegen (`tono gen`), rewrite the canonical module name before
this mapping:

- `--flatten` collapses the dotted hierarchy into one flat segment (dots become
  underscores), so no sub-packages are produced (`payments.common` ->
  `payments_common`).
- `--module-remap <from>=<to>` rewrites a matching dotted prefix (repeatable);
  `--module-remap payments=billing` turns `payments.common` into `billing.common`.

Private items are never part of a package's public export surface. The mapping is
purely structural; in-code identifiers are always the local (last) name, so
qualifying a module never changes a generated type or field name.

## Regenerating the fixtures

```
dune exec frontend/tools/dump_fixtures.exe -- write ir-schema/fixtures
```

The golden gate (`dune test`, and the cross-language round-trip) fails if a
checked-in fixture no longer matches the encoder output.
