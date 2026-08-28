# The FFI bench: `mathkit`

A fictional numeric library declared with the shapes real libraries have, and
the `.tono` that binds it. It exists because every FFI fixture so far was
designed together with the emitter it exercised, agreed with it by
construction, and then a real consumer found a defect in each: an injected
handle that never compiled (Go), a library composing its own handles (Go),
`yields` on a handle method (TypeScript), a non-`Clone` handle and a
parameter named differently from its field (Rust).

This bench inverts the order. It was written before any emitter could pass
it, from the shape of the libraries, and it is what an FFI change has to get
through. The criterion is not "generates": it is compiles and runs against
the stand-in library.

The domain is fictional and self-contained on purpose: nothing here names a
real library, package, or vendor.

## The contract

A `Calculator<T>` produces one value. Three constructors, each with a
different argument shape, and a combinator that composes handles already
built:

- `constant`: no options, one scalar.
- `formula`: an expression plus options (functional and variadic in Go, a
  struct by value in Rust, an optional object in TypeScript).
- `series`: a collection of values.
- `fallback`: N already-built handles, returning a handle of the same
  contract.

A handle exposes `compute()`: `Compute(ctx)` in Go, a future in Rust,
synchronous in TypeScript.

## The three stand-in libraries

| target | where | shape kept on purpose |
|---|---|---|
| Go | `ext/go` | `Calculator[T]` is an interface; `FromFormula(expr, opts ...Option)` with `WithPrecision(int) Option`; `FromFallback(strategy, calcs ...Calculator[T])`; a `Client` type (the name every generated entry takes too) with `Open` returning `(*Client, error)` and `Dial` returning only `*Client`; `Memo[T]`, generic over a type the library never sees; `Tuning[T]`, an interface resolved from the environment by reflection over the `env` struct tag of each field of `T` (`TuningFromEnv[T](service, opts ...EnvOpt)` with `WithParam` substituting `{name}` in a variable name at run time, `TuningPinned`, `TuningFallback`) |
| Rust | `ext/rust` | `Calculator<T>` is a trait; `from_constant`, `from_formula`, `from_series`, `from_fallback` each return a different concrete struct; `FormulaOptions { precision: Option<u8> }` by value; `Vec<Box<dyn Calculator<T>>>`; no handle is `Clone`; a `Client` type; `Memo<T>` |
| TypeScript | `ext/ts` | every constructor is a class (`new`); `FormulaOptions` is an optional object; `Calculator<T>[]`; `compute()` is synchronous; a `Client` class; `Memo<T>` |

## Files

- `service.tono`: the bench proper. Declares the whole contract for the
  three targets and composes it: three constructors of different shapes, the
  fallback receiving two constructed handles, the results feeding two
  operations, plus the declared tests that stub every foreign call. Two
  rules carry the file: `#(...)` is a foreign spelling, emitted verbatim and
  never text; and everything specific to one language lives in that
  language's block, at every level (the ext header, a struct, a field, an
  op, the error struct).
- `probes/NN-*.tono`: one small single-target file per capability, so the
  gate can report each capability on its own instead of one red for
  everything. `10-ownership-refused.tono` is the negative one: it must be
  refused at generation.
- `ext/`: the three stand-in libraries, next to the spec that binds them,
  the same layout as the other examples with an `ext` block.
- `verify/`: the drivers that run the generated SDK against the stand-in
  library for real, one per target, unreachable until the bench builds.
- `gate.tsv`: the record of what each check must reach today. The gate
  compares against it in both directions.
- Every row runs `tono check` against the stand-in libraries before
  generating, with the Go module and the `node_modules` the gate builds for
  it: a binding that diverges from the library stops the row as
  `check-red`, at the `.tono` line, before any generated file exists.
- `scripts/check-ffi-bench.sh`: the gate, called from
  `scripts/check-example-compiles.sh`.

## State of the capabilities

Measured by `scripts/check-ffi-bench.sh` (run it to reproduce; every row
prints the tool output that stopped it). Outcomes: `pass` (compiles, tests
run, driver runs), `frontend-red`, `check-red` (`tono check` found a binding
that diverges from the stand-in library, reported on the `.tono`), `gen-red`,
`build-red`, `test-red`, `run-red`, `refused`.

| # | capability | where it shows | check | state today |
|---|---|---|---|---|
| 1 | a foreign handle that is an interface, not a pointer to a struct | Go `Calculator[T]` | `01-go-interface-handle` | pass: the handle's `go` block spells the whole storage type, `#(Calculator[float64])`, without `*`, and the SDK holds the interface value itself |
| 2 | the foreign type name per language | `Calculator` (Go, TS) vs `ConstantCalculator` (Rust) | `02-rust-foreign-name` | pass: each language block spells its own storage type; a handle held as the concrete `#(ConstantCalculator<f64>)` reaches the trait method through its path, `call: #(Calculator::compute)()`, which is what brings the trait into scope |
| 3 | several concrete types for one logical handle | Rust: four structs, one trait | `03-rust-concrete-types` | pass: the Rust block spells `#(Box<dyn Calculator<f64>>)`, so each constructor's concrete value is boxed where it is built and the concrete types never need a tono name |
| 4 | a variadic parameter | `opts ...Option`, `calcs ...Calculator[T]` | `04-go-variadic-options` | pass: the logical parameter is a collection (`calcs: []calculator`) and the Go block spells what it crosses as, `calcs: #(...Calculator[float64])`, which spreads the caller's list into the variadic slot |
| 5 | a collection of handles as an argument | `Vec<Box<dyn Calculator<T>>>`, `Calculator<T>[]` | `05-rust-handle-collection` | pass: the same collection parameter, spelled `#(Vec<Box<dyn Calculator<f64>>>)` in the Rust block, collects the already-built handles |
| 6 | a nested call in `call:` | `WithPrecision(4)` inside `FromFormula` | `06-nested-call` | pass: a spelling immediately followed by `(` in a `call:` argument is a nested foreign call, `#(WithPrecision)(4)` |
| 7 | a struct literal in `call:` | `FormulaOptions { precision: 4 }` | `07-rust-struct-literal` | pass: the form's Rust block names the library's own type and spells the field, `rust { #(FormulaOptions)  precision: #(Option<u8>) }`, so the literal renders as `FormulaOptions { precision: Some(..) }` |
| 8 | construction by `new`, not a function call | TypeScript classes | `08-ts-new-construction` | pass: the callee spelling carries the `new`, `call: #(new ConstantCalculator)(value)` |
| 9 | a method synchronous in one target, asynchronous in the others | `compute()` in TS | `09-ts-sync-method` | pass: `@async(rust)` lists the targets where the foreign call is asynchronous; absence means synchronous, so TypeScript's `compute()` is neither awaited nor declared as a `Promise` |
| 10 | a handle composed and read separately | fallback + a read of one of its inputs | `10-ownership-refused` | refused, as intended: single ownership is the rule, and the generator names the field and both readers |
| 11 | a static method as the constructor: the call's receiver is the foreign type itself | `FormulaCalculator.parse(expr)` (TS), `FormulaCalculator::parse(expr)` (Rust) | `11-ts-static-method`, `11-rust-static-method`, `11-go-package-function` | pass: the receiver is inside the spelling (`#(FormulaCalculator.parse)`, `#(FormulaCalculator::parse)`), imported in TypeScript and crate-qualified at its head in Rust; Go has no static method and nothing to refuse: its block writes the package function the library exposes there |
| 12 | a class reference as an argument: the library takes the class itself and constructs it | `instantiate(AnswerCalculator)` (TS, `new () => T`) | `12-ts-class-reference`, `12-rust-class-reference-refused`, `12-go-class-reference-refused` | pass: a bare name in a `call:` argument that is a handle of the same ext block (and no parameter) passes its class, the head of the handle's `ts` storage type; Rust and Go have no type as a value, so their generation refuses the binding naming the site (refused, as intended) |
| 13 | a foreign type whose name collides with a generated one | the library's `Client` next to the generated `Client` | `13-go-name-collision`, `13-rust-name-collision`, `13-ts-name-collision` | pass: every word of a spelling is the library's, never matched against the module's own types, so `#(*Client)` stays `mathkit.Client` (Go), `mathkit::Client` (Rust), an import from the library (TypeScript) whatever the module generates under the same name |
| 14 | a generated type inside a foreign spelling | `Memo[T]` instantiated over the tono struct `reading` | `14-go-generated-type-reference`, `14-rust-generated-type-reference`, `14-ts-generated-type-reference` | pass: the module's own type enters a spelling only as an explicit reference, `#(*Memo[.reading])`, `#(Remember[.reading])`, rendered as the generated `Reading` and never qualified; a reference no type answers is refused at generation, naming the site |
| 15 | an argument that must convert at the boundary, not just be respelled | an `i64` into `new ConstantCalculator(seed: number)` (TS), `WithPrecision(digits int)` (Go) | `15-ts-argument-coercion`, `15-go-argument-coercion`, `15-rust-numeric-refused` | pass: the spelling asks for the conversion and the generated call writes it, `Number(seed)` in TypeScript (the bigint divide), `int(digits)` in Go, mirrored inside the `tono check` probe so check and build grade the same expression; Rust writes no implicit numeric narrowing, so its generation refuses the binding naming both types (refused, as intended) |
| 16 | a pointer to a struct literal: the constructor takes its options by address | `Connect(opt *Options)` (Go) | `16-go-pointer-argument` | pass: the form's block keeps the type, `go { #(Options) }`, and the argument spells the crossing, `options { Addr: addr, Greeting: greeting }: #(&Options)`, the same annotation a parameter takes; the generated call writes `&mathkit.Options{..}` and the `tono check` probe mirrors it while still probing the form as a value, so the form and the call site are graded together (a `&` on the block satisfied the call and broke the form's probe) |
| 17 | the value comes from a method of the object the call returned, not from the call | `Read(ctx, key) *Reading` + `Reading.Result() (string, error)` (Go) | `17-go-value-from-returned-object`, `17-ts-value-from-returned-object-refused`, `17-rust-value-from-returned-object-refused` | pass: the `call:` line chains the method on the returned object, `#(Read)(#(ctx context.Context), key).#(Result)()`, one link and always a call; Go writes it as one expression and the return convention describes the last link, mirrored in the `tono check` probe; Rust and TypeScript refuse the binding naming the chained method, since `@async` names one call and a chain has two (refused, as intended) |
| 18 | a constructor that returns only the handle, no error channel | `Dial(addr) *Client` next to `Open(addr) (*Client, error)` (Go) | `18-go-infallible-constructor` | pass: omitting `yields:` means the target's whole convention (`open` writes nothing and binds `(T, error)`); a `yields:` with no `returns:` is the call's whole signature, so `yields: (c: session)` binds exactly one value, the position consumed by the op's own return, with no `returns:` (a handle is never projected: writing one is refused) and no implied error; the `tono check` probe binds the same one value |
| 19 | a generic with a bound, and a composition that hands two handles back to the library | `Bounded<T extends object>` + `boundedFallback(a, b)` (TypeScript) | `19-ts-bounded-generic` | pass: the handle's generated interface declares each method's return as the `.tono` declares it (`read(): Promise<Profile>`), so `T` instantiates from the handle field itself; an interface answering `unknown` could not satisfy the bound, and no annotation at the call repairs that (`tono check` passed while `tsc` refused the generated composition, which is why the gate is the target compiler). Go's generated interface already speaks the declared return, and Rust generates no type for a handle (its field holds the `rust` block's spelling verbatim), so neither has a synthesized return to get wrong |
| 20 | a class reference to one of the module's own structs: the library constructs the caller's type and fills it | `instantiateInto(name, clazz: new () => T, mappings)` (TypeScript) | `20-ts-class-reference-generated-struct`, `20-go-class-reference-generated-struct-refused`, `20-rust-class-reference-generated-struct-refused` | pass: a bare name in a `call:` argument may also be a wire struct of the module (non-generic, and no parameter of the op), `#(instantiateInto)(name, profile, mappings)`; a struct is an interface in TypeScript, which has no value at run time, so the types file also declares `export class Profile {}` under the same name (TypeScript merges the two: the class is the constructor the library gets, the interface stays the shape), the ext glue imports it from the module, and the barrel keeps re-exporting `Profile` as a type only, so the SDK's public surface does not change; the `tono check` probe skips the binding, since the class is the SDK's own, and the target compiler grades the generated call; Rust and Go have no type as a value, so their generation refuses the binding naming the struct (refused, as intended). The mappings table is a `Map<string, Mapping>` on the library's side while the generated map type is a plain object, in both directions: the parameter spells the crossing, `mappings: #(Map<string, .mapping>)`, and the call writes `new Map(Object.entries(mappings))`; the method reading the table back spells what the library answers, `yields: (table: #(Map<string, .mapping>))`, so the handle interface declares the `Map` and the call site writes `Object.fromEntries(raw)`. A spelled answer TypeScript cannot convert back is refused naming both types, the way an argument spelling is. Measured first with the class reference alone: `tono check` green (it skips sites referencing generated types), `tsc` on the generated code red in both directions (TS2740 at the argument, TS2352 on the handle cast), which is the check-green/build-red class this bench exists to catch |
| 21 | a wrong binding inside the family that names a generated type, refused by the check before anything is generated | the handle `Memo<.reading>` / `*Memo[.reading]` with `recall(): string` where the library answers the Reading | `21-ts-generated-type-wrong-binding`, `21-go-generated-type-wrong-binding` | check-red, as intended: the check compiles its probe beside the SDK's own type declarations (generated in memory by the same pipeline `tono gen` runs, only each module's types file and the root support it imports), in the module's own directory or package, so a spelling that references a generated type, a parameter typed by one, or a struct passed as a class is graded like any other binding, and the finding lands on the method's span in the `.tono` (before this, the whole family was listed as unchecked and the mismatch was only found by the target compiler on a generated line, the check-green/build-red class capabilities 19 and 20 first showed) |
| 22 | a literal of one of the module's own structs, built inline as the argument of a foreign call | `Remember[T any](value T)` over the generated `Reading`, called as `remember(reading { value: .value, label: .label })` (all three) | `22-go-generated-struct-literal`, `22-rust-generated-struct-literal`, `22-ts-generated-struct-literal` | pass: capability 14 passes the same value by reference to a field (`remember(.seed)`); the literal is that value written where it is passed, so the two bindings generate the same call and differ only in the argument expression. A literal naming none of the lib's forms builds one of the module's wire structs, rendered as the generated type's own literal (`Reading{Label: label, Value: value}` in Go, `Reading { label: label, value: value }` in Rust, the object literal with the interface's own keys in TypeScript), the type and fields named as the types file names them, no foreign block looked for. Measured first with the literal alone: TypeScript green (an object literal is structural, so the emitter never looked for a form), Go and Rust dead at generation, the emitter asserting the validator had refused a form with no block, which the validator never saw because the name was not a form; now the validator decides: a generic struct, an entry or an enum, a field the struct does not declare, a required member left out, or a spelling on the literal is refused naming the site, and a non-generic wire struct with its required members is built |
| 23 | a struct tag on a generated type: the library reads the module's own struct by reflection, at run time | `Tuning[T]` + `TuningFromEnv[T](service, WithParam("profile", ..))` over the tono struct `calibration` (Go) | `23-go-struct-tags` | pass: a wire struct's `go` block has no head and declares one Go struct tag per field, verbatim, `go { scale: #(env:"{service}_{profile}_SCALE") }`, which the emitter appends after the `json` tag it derives (`json:"scale" env:"..."`); a field without an entry keeps only the derived tag, and a declared key the emitter derives itself (`json`) is refused at generation naming both tags, never merged. Every binding here (generic instantiation over a generated type, an interface handle by value, the variadic fallback, a functional option) passed before this capability and the generated code compiled: a tag is invisible to the compiler, so the driver is the gate. It sets the variables (one with `{profile}` substituted by `WithParam`), reads the calibration back through the generated client, and reads the pinned defaults through the fallback for a profile the environment lacks; the same probe without the block ends `run-red`, never `build-red`. TypeScript's equivalent forms bind without a per-field declaration, and Rust waits for a real case, so a `ts` or `rust` block on a wire struct is refused by the checker rather than guessed at |

Every capability passes today, capabilities 10, 15 (Rust), 17 (Rust and
TypeScript) and 20 (Rust and Go) by being refused (each a rule, not a gap),
and capability 21 by being refused by `tono check` on the `.tono`.
Capability 23 is the one graded by its driver alone: the target compiler
cannot see a struct tag.

What `tono check` leaves unchecked across the rows, and why, after the
probe started compiling beside the generated types: every Rust binding
(reading a crate's signatures needs rustdoc JSON, nightly only), the
chained call in TypeScript (17, the emitter refuses it too), the class
reference in Go (12 and 20, the emitter refuses it too), and the bindings
of a row generation refuses (20 on Go: the types are not generated, so the
sites that need them are listed with the refusal). Nothing is left
unchecked for naming a type the generated SDK defines, and the gate fails a
row whose report says so.

### The bench proper (`service.tono`)

The full three-target file compiles through the frontend, and all three
targets generate, build, run their declared tests and the driver (`bench
typescript`, `bench rust`, `bench go`: pass). The last two Go blockers were
not among the capabilities; the bench found them next, and each is now
emitted: an error only an `ext` op declares (here `invalid_expression`,
raised by `from_formula` at construction, never by an operation on the wire)
gains the same `Error()` method a wire-declared error has, so the
constructor can return `&InvalidExpression{..}` as an `error`; and a
declared test stubbing several constructors of one handle type declares one
fake for that type, since the fake answers by handle method, not by which
constructor built it.

### The error struct

`invalid_expression` is an ordinary tono error struct whose language blocks
say how each target recognizes the library's failure and where each field
comes from: `go { #(ErrParse)  message: #(Error()) }` (by identity,
`errors.Is`; a pointer type such as `#(*ParseError)` is matched by type,
`errors.As`) and `rust { #(Error::Parse)  message: #(to_string()) }` (by
pattern). The op only lists it, `@errors(invalid_expression)`, in test
order. TypeScript recognizes a class with `instanceof`; the bench library
throws no class, so it declares no `ts` block.

## Call-argument shapes not covered here

One shape a consumer raised was deliberately not attempted as a small
extension of `call:`'s own argument grammar:

- **Map literal** as a call argument needs its own key-value collection
  shape. tono already has `map[K]V` and the `{ "k": v }` literal used in
  test headers, so the question is which of those a binding reaches for,
  not a new construct. Capability 20 is the consumer form that carries one
  (`instantiateInto(name, clazz, mappings)`); the bench passes the table as
  a map-typed parameter instead, which is the value crossing (into the
  library's `Map`, and back) without the literal.

It is its own scoped piece of work with its own design questions.

## Updating the record

When an emitter change makes a check end somewhere else than `gate.tsv`
says, the gate fails and names the row. Progress is recorded by moving that
row's expected outcome forward and updating the table above; a check that
reaches `build` needs its driver next to it (`probes/<id>.verify.{go,rs,test.ts}`,
or `verify/` for the bench proper) or the gate reports `no-driver`. Never
make a check pass by changing the stand-in library to fit the emitter: the
library's shape is the point.
