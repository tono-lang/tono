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
- `table`: a collection keyed by name (a map of string to value).
- `fallback`: N already-built handles, returning a handle of the same
  contract.

A handle exposes `compute()`: `Compute(ctx)` in Go, a future in Rust,
synchronous in TypeScript.

## The three stand-in libraries

| target | where | shape kept on purpose |
|---|---|---|
| Go | `ext/go` | `Calculator[T]` is an interface; `FromFormula(expr, opts ...Option)` with `WithPrecision(int) Option`; `FromTable(entries map[string]T)`; `FromFallback(strategy, calcs ...Calculator[T])` |
| Rust | `ext/rust` | `Calculator<T>` is a trait; `from_constant`, `from_formula`, `from_series`, `from_table`, `from_fallback` each return a different concrete struct; `FormulaOptions { precision: Option<u8> }` by value; `HashMap<String, T>`; `Vec<Box<dyn Calculator<T>>>`; no handle is `Clone` |
| TypeScript | `ext/ts` | every constructor is a class (`new`); `FormulaOptions` is an optional object; `Record<string, T>`; `Calculator<T>[]`; `compute()` is synchronous |

## Files

- `service.tono`: the bench proper. Declares the whole contract for the
  three targets and composes it: three constructors of different shapes, the
  fallback receiving two constructed handles, the results feeding two
  operations, plus the declared tests that stub every foreign call. Where
  the language cannot express a shape yet, a comment names the capability
  below and the nearest form is declared instead of bending the library.
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
- `scripts/check-ffi-bench.sh`: the gate, called from
  `scripts/check-example-compiles.sh`.

## State of the capabilities

Measured by `scripts/check-ffi-bench.sh` (run it to reproduce; every row
prints the tool output that stopped it). Outcomes: `pass` (compiles, tests
run, driver runs), `frontend-red`, `gen-red`, `build-red`, `test-red`,
`run-red`, `refused`.

| # | capability | where it shows | check | state today |
|---|---|---|---|---|
| 1 | a foreign handle that is an interface, not a pointer to a struct | Go `Calculator[T]` | `01-go-interface-handle` | pass: the `interface` marker on the handle declaration drops the pointer, and the SDK holds the interface value itself |
| 2 | the foreign type name per language | `Calculator` (Go, TS) vs `ConstantCalculator` (Rust) | `02-rust-foreign-name` | build-red, one step further: the instantiation now names the type per language (`type calculator(rust: "ConstantCalculator", float)`), verbatim, and the type resolves; what remains is that `compute` is a trait method on the concrete type and nothing brings the trait into scope |
| 3 | several concrete types for one logical handle | Rust: four structs, one trait | `03-rust-concrete-types` | pass: the `interface` marker makes the handle the trait; Rust holds `Box<dyn Calculator<f64>>` and boxes each constructor's concrete value where it is built, so the concrete types never need a tono name |
| 4 | a variadic parameter | `opts ...Option`, `calcs ...Calculator[T]` | `04-go-variadic-options` | pass: the `variadic` marker on the logical parameter's type spreads a call-site list as `[]Option{...}...` |
| 5 | a collection of handles as an argument | `Vec<Box<dyn Calculator<T>>>`, `Calculator<T>[]` | `05-rust-handle-collection` | pass: a `variadic` parameter's call-site list collects the already-built handles into the collection the library expects |
| 6 | a nested call in `call:` | `WithPrecision(4)` inside `FromFormula` | `06-nested-call` | pass: a string immediately followed by `(` in a `call:` argument is a nested foreign-symbol call |
| 7 | a struct literal in `call:` | `FormulaOptions { precision: 4 }` | `07-rust-struct-literal` | build-red: accepted by the frontend; Rust cannot be told the foreign struct's own name nor wrap the value in `Some` (blocked first by 3) |
| 8 | construction by `new`, not a function call | TypeScript classes | `08-ts-new-construction` | pass: the `new` marker on the language binding constructs with `new Symbol(args)` instead of calling it plainly |
| 9 | a method synchronous in one target, asynchronous in the others | `compute()` in TS | `09-ts-sync-method` | pass: the `sync` marker now reaches the generated handle interface's own method signature, not just the call site |
| 10 | a handle composed and read separately | fallback + a read of one of its inputs | `10-ownership-refused` | refused, as intended: single ownership is the rule, and the generator names the field and both readers |
| 11 | a static method as the constructor: the call's receiver is the foreign type itself | `FormulaCalculator.parse(expr)` (TS), `FormulaCalculator::parse(expr)` (Rust) | `11-ts-static-method`, `11-rust-static-method`, `11-go-static-method-refused` | pass: the type is a second string before the method (`call: "FormulaCalculator"."parse"(expr)`), imported in TypeScript and qualifying the path in Rust; Go has no static method, so its generation refuses the binding naming the site (refused, as intended) |
| 12 | a class reference as an argument: the library takes the class itself and constructs it | `instantiate(AnswerCalculator)` (TS, `new () => T`) | `12-ts-class-reference`, `12-rust-class-reference-refused`, `12-go-class-reference-refused` | pass: `type answer_calculator` in a `call:` argument names a declared handle and passes its class, imported in TypeScript; Rust and Go have no type as a value, so their generation refuses the binding naming the site (refused, as intended) |
| 13 | a map literal as an argument: the library takes a collection keyed by name | `FromTable(map[string]T)` (Go), `from_table(HashMap<String, T>)` (Rust), `new TableCalculator(Record<string, T>)` (TS) | `13-go-map-literal`, `13-rust-map-literal`, `13-ts-map-literal` | pass: `{ "answer": .answer, "other": 1.5 }` in a call argument is the value side of a `map[string]V` logical parameter; Go types the literal by that parameter (`map[string]float64{...}`), Rust renders `HashMap::from([...])` over owned `String` keys, TypeScript an object literal with quoted keys |

Passing today: capabilities 1, 3, 4, 5, 6, 8, 9, 11, 12 and 13, plus
capability 10 (the one that is a rule, not a gap). Two gaps remain (2 and 7, both blocked on
per-language foreign-type naming reaching Rust's trait scope and `Some`
wrapping, a separate capability).

### The bench proper (`service.tono`)

The full three-target file compiles through the frontend; Go stops at
generation (its declared-test emitter does not carry what the bench needs)
while TypeScript and Rust generate and stop at the capabilities:

- Go (`bench go`, gen-red): a `[]float @arg` pinned in a test renders as
  `[]float64("")`, and four stubs of the same handle type in one test emit
  the same fake type four times.
- TypeScript and Rust (`bench typescript`, `bench rust`, build-red): the
  handle-method stub generates now (the emitter used to panic on
  `stub mathkit.calculator.compute`), so both targets reach the build and
  stop at the capabilities the probes already report (2 and 8 first).

The Go blocker is not among the ten capabilities; it is the first thing the
bench found. Once it clears, the Go bench too will stop at the capabilities
the probes already report.

## Call-argument shapes that were once not covered here

Three shapes a consumer raised were deliberately not attempted as a small
extension of `call:`'s own argument grammar; each became a capability on its
own terms. The static method (11) turned out to be a third kind of call
receiver (alongside a declared `ext` namespace, `ns.fn(..)`, and a handle
field, `.field.method(..)`), and got its own syntax, the receiver type as a
second string before the method. The class reference (12) turned out not to
need a type-level construct at all: tono never constructs or inspects the
class, so what crosses the boundary is only a foreign name, and a declared
handle already carries that name per language; `type handle` in a `call:`
argument reads the handle's foreign name in value position. The map literal
(13) is the key-value sibling of the list: a value shape of the call
argument, not a reuse of the wire `map[K]V` type, for the same reason the
list never reused the wire `[]T`. A wire type describes what a wire carries;
a call argument's items are foreign values (a parameter, a handle, a nested
call) that no wire type names. The logical parameter's declared type is
still the wire type (`entries: map[string]float`), and that is what Go reads
to type the literal, the way a variadic parameter types its spread. Keys are
string literals until a real case asks for more.

## Updating the record

When an emitter change makes a check end somewhere else than `gate.tsv`
says, the gate fails and names the row. Progress is recorded by moving that
row's expected outcome forward and updating the table above; a check that
reaches `build` needs its driver next to it (`probes/<id>.verify.{go,rs,test.ts}`,
or `verify/` for the bench proper) or the gate reports `no-driver`. Never
make a check pass by changing the stand-in library to fit the emitter: the
library's shape is the point.
