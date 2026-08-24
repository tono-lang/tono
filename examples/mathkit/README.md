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
| Go | `ext/go` | `Calculator[T]` is an interface; `FromFormula(expr, opts ...Option)` with `WithPrecision(int) Option`; `FromFallback(strategy, calcs ...Calculator[T])`; a `Client` type (the name every generated entry takes too); `Memo[T]`, generic over a type the library never sees |
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
| 16 | a pointer to a struct literal: the constructor takes its options by address | `Connect(opt *Options)` (Go) | `16-go-pointer-argument` | pass: the form's block keeps the type, `go { #(Options) }`, and the argument spells the crossing, `options { Addr: addr }: #(&Options)`, the same annotation a parameter takes; the generated call writes `&mathkit.Options{..}` and the `tono check` probe mirrors it while still probing the form as a value, so the form and the call site are graded together (a `&` on the block satisfied the call and broke the form's probe) |

Every capability passes today, capabilities 10 and 15 (Rust) by being
refused (each a rule, not a gap).

### The bench proper (`service.tono`)

The full three-target file compiles through the frontend; TypeScript and
Rust generate, build, run their declared tests and the driver (`bench
typescript`, `bench rust`: pass). Go stops at generation (`bench go`,
gen-red): its declared-test emitter does not carry what the bench needs (a
`[]float @arg` pinned in a test renders as `[]float64("")`, and four stubs of
the same handle type in one test emit the same fake type four times). That
blocker is not among the fourteen capabilities; it is the first thing the bench
found, and the probes report every Go capability green around it.

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
  not a new construct.

It is its own scoped piece of work with its own design questions.

## Updating the record

When an emitter change makes a check end somewhere else than `gate.tsv`
says, the gate fails and names the row. Progress is recorded by moving that
row's expected outcome forward and updating the table above; a check that
reaches `build` needs its driver next to it (`probes/<id>.verify.{go,rs,test.ts}`,
or `verify/` for the bench proper) or the gate reports `no-driver`. Never
make a check pass by changing the stand-in library to fit the emitter: the
library's shape is the point.
