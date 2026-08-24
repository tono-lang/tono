# IR JSON encoding

The serialized IR is the wire contract between the OCaml frontend (the source of
truth) and the Rust backend (the mirror). This document defines the encoding so
both sides reference one source. The golden fixtures under `fixtures/` are the
arbiter: they are generated from the frontend encoder and decoded by the backend,
and any divergence breaks the build.

## Version envelope

The top-level document is:

```json
{ "tono_ir_version": 20, "modules": [ /* module objects */ ] }
```

`tono_ir_version` is a single monotonic integer, not a semantic version. It is
bumped by one on every incompatible change to this encoding. A decoder that sees
a version it does not recognize fails loudly rather than attempting a partial
decode; there is no negotiation or multi-version support.

The current version is **28**.

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
`traits`. There are exactly seven kinds: five wire kinds plus the two
construction kinds (`entry` and `config`, below).

```json
{ "id": "payments#Charge", "kind": "structure",
  "params": [], "members": [ /* members */ ], "traits": [] }

{ "id": "payments#Source", "kind": "union",
  "params": [], "members": [ /* members */ ], "discriminator": "type", "traits": [] }

{ "id": "payments#Status", "kind": "enum",
  "backing": "string",
  "values": [ { "name": "active", "traits": [] }, { "name": "closed", "traits": [] } ],
  "traits": [] }

{ "id": "payments#Payments", "kind": "service",
  "operations": [ "payments#ListCharges" ], "traits": [] }

{ "id": "payments#ListCharges", "kind": "operation",
  "input": null, "output": { "ref": "core#Page", "args": [ /* ... */ ] },
  "errors": [], "traits": [],
  "wire": { /* see "Resolved wire bindings" below; omitted when there is no @http trait */ } }
```

- `union` always emits an explicit `discriminator` (default `"type"`).
- `enum` carries `backing` (`"string"` or `"int"`) and `values`, each an object
  `{ "name", "value"?, "traits" }`: `value` is the explicit integer, present only
  on an int-backed enum, and `traits` is the member's bag (documentation rides it).
  Every enum is open; the implicit unknown variant is a decode-time concern of the
  backend and is not materialized here.
- Documentation is the `@doc` trait (`core#doc`), a Markdown string carried in any
  trait bag (shape, member, or enum value); the backend lowers it to each target's
  native doc comment.
- `operation` carries `input`/`output` as a type reference or `null`, and
  `errors` as an array of type references. They are type references (not bare
  ids) so an operation can return an applied generic directly.
  `output_nullable: true` (v29) marks a declared `T?` return, the same
  two-state rule as a member's `required` (nullability is not a type node);
  the key is omitted when false. `wire` (the
  resolved HTTP binding, see below) is present only on an operation with an
  `@http` trait; the key is omitted otherwise. `impl_call` (v15) is present
  only on an operation with its own `impl .field.method(args)` body
  `{ "recv": ["bus"], "method": "send", "args": [ /* call
  arguments */ ] }`, where `recv` is the field path and `args` use the same
  call-argument encoding as an "ext" library's `call:` line (see "FFI
  library declarations" below). It is a third implementation source
  alongside `wire` and a legacy `impl` extension; an operation carries at
  most one of the three.

## Entries and configs (v5)

A struct with ops in its body is an `entry` (the SDK construction surface plus
its methods); a struct that only participates in construction is a `config`.
Neither is a wire type. An entry nests its operations as full shape objects,
identified `module#entry.op` so they never collide with top-level shapes:

```json
{ "id": "notes#client", "kind": "entry",
  "fields": [ /* entry fields */ ],
  "operations": [ /* operation shapes */ ], "traits": [] }

{ "id": "notes#conf", "kind": "config",
  "fields": [ /* entry fields */ ], "traits": [] }
```

An entry field replaces the member's `required`/`default` pair with declared
value sources (presence is governed by the sources):

```json
{ "name": "endpoint_v2",
  "target": { "prim": "string" },
  "sources": [ "with", { "env": "ENDPOINT" },
               { "env": { "field": ["endpoint_env"] } }, { "default": "v2" } ],
  "format": [ { "lit": "ENDPOINT_" }, { "field": ["client_key"] }, { "lit": "_V2" } ],
  "transforms": [ "trim", "upper_snake" ],
  "select": { "subject": ["endpoint_version"],
              "arms": [ { "pattern": "v1", "value": { "field": ["endpoint_v1"] } },
                        { "value": { "sources": [ { "env": "ENDPOINT_V2" } ] } } ] },
  "binds": [ { "field": "api_key", "source": ["api_key"] } ],
  "constraints": [], "traits": [] }
```

- `sources` is the declared fallback chain, in order. The forms are `"arg"`,
  `"with"`, `{ "env": <string | {"field": [...]}> }`, and
  `{ "default": <json> }`; `"arg"` is exclusive and never stacks with the
  others (the typechecker rejects the combination as dead sources).
- `format` (omitted when absent) is the parsed `@format` template: `lit`
  literal runs, `field` entry-field placeholders (`{.x}`), and `input`
  operation-input placeholders (`{id}`, protocol trait positions only).
- `transforms` is the `@str::*` pipeline in declared order (bare names,
  e.g. `"trim"`).
- `select` (omitted when absent) is the `= match` table: `subject` is a field
  path; each arm carries a scalar JSON `pattern` (absent = wildcard) and a
  `value` that is one of `{"field": [...]}`, `{"lit": <json>}`, or
  `{"sources": [...]}`.
- `call` (omitted when absent, v14) is a field's `= ns.fn(args)` extern-call
  source: `{"ns": <string>, "fn": <string>, "args": [<call_arg>]}`. See
  "FFI library declarations" below for `call_arg` and the rest of the
  `ext`/`extern` surface it shares.
- `handle_call` (omitted when absent, v22) is a field's `= .field.method(args)`
  handle-method-call source: `{"recv": ["provider"], "method": "get", "args":
  [<call_arg>]}`, the same shape as an operation's `impl_call`. The receiver
  is a sibling entry field typed by a declared opaque handle; the field's
  value is what the method returns, so one foreign resolution can feed
  several operations. A field carries at most one of `select`, `call`, and
  `handle_call`.
- `binds` are the `@bind(target, .source)` pairs of a composed config field.

A field reference inside an operation trait value is the structured object
`{"field": ["a", "b"]}` (e.g. `@http`'s `endpoint:`, `@header` values,
`@timeout`/`@retry`); path templates keep both placeholder scopes verbatim in
the string (`"/notes/{.x}/{id}"`).

## FFI library declarations (v14, reshaped in v28)

The module `ext_libs` array is the `ext <name> { ... }` library-block form,
distinct from the legacy `extensions` table above (contract/constraint/impl).
Every string that is not a tono name is a foreign spelling, the text of a
`#(...)` on the surface, carried verbatim. One entry per `ext` block:

```json
{ "name": "mathkit",
  "langs": [ { "lang": "go", "path": "tono-ext-fixture/mathkit" },
             { "lang": "rust", "path": "mathkit" } ],
  "structs": [ { "name": "formula_options",
                 "fields": [ { "name": "precision", "type": { "prim": "u8" } } ],
                 "langs": [ { "lang": "rust", "name": "FormulaOptions",
                              "fields": { "precision": "Option<u8>" } } ] } ],
  "types": [ { "name": "calculator",
               "langs": [ { "lang": "go", "name": "Calculator[float64]" },
                          { "lang": "rust", "name": "Box<dyn Calculator<f64>>" } ],
               "methods": [ /* extern_decl */ ] } ],
  "externs": [ /* extern_decl */ ] }
```

- A `foreign_lang` is one language's block on a struct:
  `{"lang", "name", "fields"?}`. `name` is the positional first element of
  the block and what it is depends on the struct: a foreign form's type, an
  opaque handle's whole storage type, an error struct's sentinel or error
  type. `fields` (omitted when empty) pairs a tono field name with its
  foreign spelling: the field's foreign type on a form, where the field
  comes from on an error value (`"message": "Error()"`).
- `structs` are foreign forms declared inside the block, field names/casing
  kept verbatim (never normalized); never a top-level shape, never role-
  classified, never crosses the wire. Each carries its `langs`; a target
  with no block does not have the form.
- `types` are opaque foreign handles; `langs` spells the storage type per
  language (nothing is derived from the handle's name), and each `methods`
  entry is an `extern_decl` (a receiver method, same shape as a free extern).
- An `extern_decl` is `{"name", "params": [{"name","type"}], "return": <tref>,
  "langs": [<extern_lang>], "async"?, "errors"?}`. `async` (omitted when
  empty) lists the languages where the foreign call is asynchronous; absent
  means synchronous at the boundary. `errors` (omitted when empty) lists the
  declared error shapes the call can raise, by shape id, in test order; how
  a target recognizes each one lives on that shape (below).
- An `extern_lang` (one per language block) is
  `{"lang", "symbol", "call_args": [<call_arg>], "yields"?, "returns"?}`.
  `symbol` is the callee's whole foreign spelling (`FromConstant[float64]`,
  `new ConstantCalculator`, `FormulaCalculator::parse`). `yields` is
  omitted when empty; `returns` when absent. A `yields` position is
  `{"name", "type"?, "is_error"?, "foreign"?}`: `type` is the tono type it
  carries, absent for the reserved `error` sentinel (`is_error`: `true`)
  and for a position under a foreign spelling (`foreign`: what the call
  really returns, for the target to coerce into the declared return).
  `returns` is `{"type": <tref>, "fields": [{"name","value"}]}` where a
  field's `value` is `{"field": [...]}` (a ref into a `yields`-bound name)
  or `{"select": <select>}` (a match over one, reusing the same `select`
  shape as `= match`).
- A `call_arg` is a tagged object: `{"param": <string>}` (the extern's own
  parameter), `{"param": <string>, "as": <string>}` (the parameter under a
  foreign spelling of its own, what it crosses as: `Vec<f64>`,
  `...Calculator[float64]`), `{"foreign": <string>}` (a declared position
  the target binds itself, `ctx context.Context`), `{"field": [...]}` (a
  ref path), `{"ctor": <string>, "fields": {<name>: <call_arg>}}` (a
  struct-literal mapper), `{"lit": <json>}` (a bare scalar literal),
  `{"list": [<call_arg>]}`, `{"symbol": <string>, "symbol_args": [...]}` (a
  nested foreign call), `{"type": <string>}` (a declared handle passed as a
  class reference), or `{"call": <entry_call>}` (a call into another
  declared extern, only inside a ctor field's value). `entry_call` is
  `{"ns", "fn", "args": [<call_arg>]}`, the same shape an entry field's
  `call` key and a trait argument's own `{"call": <entry_call>}` form both
  carry.
- An error struct carries its language blocks as the `foreign` trait of its
  shape: `{"id": "foreign", "value": [<foreign_lang>]}`, one block per
  language, `name` being the sentinel (`ErrParse`, matched by identity) or
  the pointer type (`*TimeoutError`, matched by type) in Go and the pattern
  (`Error::Parse`) in Rust, the class (`ParseError`, `instanceof`) in
  TypeScript, with `fields` saying where each tono field comes from.

## Resolved wire bindings (v8)

An operation's `@http` annotations resolve once, in the frontend, into a
typed `wire` field on the operation shape (see the `operation` kind above) so
the backend can read the binding directly. This is the only wire
representation (v9 removed the `wire_descriptor` trait that carried the same
resolution as an opaque JSON blob, kept until then for backward
compatibility).

```json
{ "method": "GET",
  "uri": [ { "lit": "/charges/" }, { "input": "id" } ],
  "bindings": { "id": { "kind": "label" },
                "q": { "kind": "query", "name": "q" },
                "x_key": { "kind": "header", "name": "X-Key" },
                "body": { "kind": "payload" } },
  "response_bindings": { "trace_id": { "kind": "header", "name": "X-Trace-Id" },
                          "status": { "kind": "statusCode" } },
  "success": [ 200 ],
  "endpoint": [ "endpoint" ],
  "request_headers": [ [ [ { "lit": "X-Client" } ], { "field": [ "client_name" ] } ] ],
  "timeout": [ "timeout" ],
  "retry": [ "settings", "max_retries" ] }
```

- `method` is the uppercased HTTP method; `uri` is the path template,
  pre-parsed into the same `lit`/`field`/`input` parts as `format` above
  (`input` for a `{name}` operation-input placeholder, `field` for a
  `{.x}` entry-field placeholder).
- `bindings` maps an input member's name to where it travels in the request:
  `{"kind":"label"}`, `{"kind":"query","name":...}`,
  `{"kind":"header","name":...}`, `{"kind":"body"}` (the default, an
  unmarked member), or `{"kind":"payload"}` (the member is the whole body).
  `response_bindings` maps an output member's name the same way, restricted
  to `{"kind":"header","name":...}` and `{"kind":"statusCode"}`; an
  unmarked output member carries no entry (it is an ordinary body field).
  Both are JSON objects (member names are unique), unlike the array-of-pairs
  the legacy blob used.
- `success` is the list of status codes the operation succeeds on. Unlike the
  legacy blob's `success`, there is no type reference alongside the status:
  the output type is always the operation's own declared `output`, and every
  runtime already discarded the blob's ref on decode. Empty (v11+) means the
  operation left `code:` unset: every emitter falls back to the 2xx-range
  convention. A non-empty list (from `code: <int>` or `code: [<int>, ...]`)
  means an exact match against exactly those statuses, even ones inside 2xx.
- `endpoint`, `timeout`, and `retry` are entry-scoped: present only on an
  operation nested in an entry body, `null`/absent for a loose operation.
  Each is the plain entry-field path the `@http(endpoint:)`/`@timeout`/
  `@retry` argument named (e.g. `["settings", "max_retries"]` for a nested
  config field), for an emitter to resolve into a typed access at the call
  site. This differs from the legacy blob's `timeout`/`retry`, which
  pre-join the same path into a dotted string wrapped in a
  `{"ref": "..."}` runtime-lookup convention (`{"max": {"ref": "..."}}` for
  retry) meant for a string-keyed lookup against a `Values` map built at
  construction time.
- `request_headers` is `[[key, value], ...]`: `key` is a parsed template
  (same `lit`/`field`/`input` parts as `uri`), `value` is
  `{"lit": <json>}`, `{"field": [...]}`, `{"template": [...]}`, or (v17)
  `{"call": <wire_call>}`. Declared by op-level `@header(key, value)`;
  either a loose or an entry-nested operation may carry these.
- (v17) `{"call": <wire_call>}` is an extern call read as a `@header`/`@body`
  value (never `@query`: the URL is already finalized before a call could
  patch it in). `wire_call` is `{"ns", "fn", "args": [<wire_call_arg>]}`,
  same shape as an entry field's own `call` key. A `wire_call_arg` is
  `{"field": [...]}`, `{"param": [...]}`, `{"lit": <json>}`,
  `{"ctor": [[name, wire_call_arg], ...]}`, or the bare string `"request"` --
  the reserved marker for `.request`, the canonical already-assembled
  request. `.request` is legal only here (as a direct or ctor-nested call
  argument); an entry field's own call rejects it at typecheck instead
  (`.request` does not exist yet during construction). No emitter supports a
  call nested inside a `@body` ctor field's own value, or any call in
  `@query`, yet; `tono gen` rejects both at generation time rather than
  emitting broken output.
- The legacy blob's `errors` array (status, error shape id, `@errorCode`
  value, `@retryable` flag) has no counterpart here: the backend's typed
  error taxonomy is derived directly from the operation's own `errors` field
  (type references, see the `operation` kind above) and the referenced error
  shapes' own `@status`/`@errorCode`/`@retryable` traits, never from the
  wire binding.

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
  a cycle is a compile error. Two imports in one file that resolve to the same
  qualifier (a shared last segment, or an alias colliding with one) are a compile
  error, not a silent last-wins; disambiguate with an `as` alias.
  `import` and `as` are reserved words: a pre-existing model that used either as a
  shape or member name must rename it (or the reference will fail to parse).
- **Visibility.** `pub` on a top-level declaration makes it visible to other
  modules and rides the IR as a `pub` trait on the shape. A private (non-`pub`)
  shape is visible only within its own module; referencing it across a module
  boundary is a compile error. Within a module everything is visible without `pub`
  or an import. Visibility gates references, not emission: a private shape a public
  one folds in is still generated in its own module.

## Generated package mapping

The backend maps each module to an idiomatic sub-package per target, derived from
the dotted name:

| Module | Rust | Go | TypeScript |
|---|---|---|---|
| `payments.common` | `crate::payments::common` (`rust/payments/common.rs`) | package `common` (`go/payments/common/common.go`) | `./payments/common` (`typescript/payments/common.ts`) |

The generated output is compilable as a tree: Rust emits a `mod.rs` per directory
so `use crate::a::b` resolves; Go names each package for the module's last segment
and package-qualifies a cross-package reference (`common.Status`); TypeScript
imports each module by a path relative to the importing file (`./common`).

A single-segment module keeps the flat layout (`rust/payments.rs`). Config hooks,
consumed by codegen (`tono gen`), steer the mapping:

- `--flatten` collapses the dotted hierarchy into one flat segment (dots become
  underscores), so no sub-packages are produced (`payments.common` ->
  `payments_common`).
- `--module-remap <from>=<to>` rewrites a matching dotted prefix (repeatable);
  `--module-remap payments=billing` turns `payments.common` into `billing.common`.
- `--go-module <path>` sets the generated Go SDK's module path, prefixed onto
  cross-package imports (Go has no relative imports), e.g.
  `example.com/sdk/payments/common`. It is required for a multi-module Go SDK;
  generating one without it is rejected rather than emitting unresolvable imports.
  Two modules that share a last segment (both `*.common`) would map to the same Go
  package name and are likewise rejected (rename one, or `--flatten`).

Visibility governs cross-module *references* (a non-`pub` shape cannot be named
from another module), not whether a shape is emitted: a private shape that a
public one folds in (a union over private variant structs) is still emitted, as an
ordinary type in its own module, because the public type needs it to compile. The
mapping is purely structural; in-code identifiers are always the local (last)
name, so qualifying a module never changes a generated type or field name.

## Regenerating the fixtures

```
dune exec frontend/tools/dump_fixtures.exe -- write ir-schema/fixtures
```

The golden gate (`dune test`, and the cross-language round-trip) fails if a
checked-in fixture no longer matches the encoder output.
