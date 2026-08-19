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
| Go | `ext/go` | `Calculator[T]` is an interface; `FromFormula(expr, opts ...Option)` with `WithPrecision(int) Option`; `FromFallback(strategy, calcs ...Calculator[T])` |
| Rust | `ext/rust` | `Calculator<T>` is a trait; `from_constant`, `from_formula`, `from_series`, `from_fallback` each return a different concrete struct; `FormulaOptions { precision: Option<u8> }` by value; `Vec<Box<dyn Calculator<T>>>`; no handle is `Clone` |
| TypeScript | `ext/ts` | every constructor is a class (`new`); `FormulaOptions` is an optional object; `Calculator<T>[]`; `compute()` is synchronous |

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
  refused at generation. `06-nested-call.tono.rejected` is written in the
  form the frontend rejects today, so it does not carry the `.tono`
  extension the editor grammar gate walks; it is renamed the day it parses.
- `ext/`: the three stand-in libraries, next to the spec that binds them,
  the same layout as the other examples with an `ext` block.
- `verify/`: the drivers that run the generated SDK against the stand-in
  library for real, one per target, unreachable until the bench builds.
- `gate.tsv`: the record of what each check must reach today. The gate
  compares against it in both directions.
- `scripts/check-ffi-bench.sh`: the gate, called from
  `scripts/check-example-compiles.sh`.

## State of the ten capabilities

Measured by `scripts/check-ffi-bench.sh` (run it to reproduce; every row
prints the tool output that stopped it). Outcomes: `pass` (compiles, tests
run, driver runs), `frontend-red`, `gen-red`, `build-red`, `test-red`,
`run-red`, `refused`.

| # | capability | where it shows | check | state today |
|---|---|---|---|---|
| 1 | a foreign handle that is an interface, not a pointer to a struct | Go `Calculator[T]` | `01-go-interface-handle` | build-red: the emitter spells `*mathkit.Calculator[float64]`, a pointer to an interface |
| 2 | the foreign type name per language | `Calculator` (Go, TS) vs `ConstantCalculator` (Rust) | `02-rust-foreign-name` | build-red: one name for every target; in Rust that name is the trait, "expected a type, found a trait" |
| 3 | several concrete types for one logical handle | Rust: four structs, one trait | `03-rust-concrete-types` | build-red: the instantiation name is also re-cased (`Constantcalculator`), and one handle can only name one struct |
| 4 | a variadic parameter | `opts ...Option`, `calcs ...Calculator[T]` | `04-go-variadic-options` | build-red: no variadic form; the precision passed positionally is refused ("cannot use uint8 as Option") |
| 5 | a collection of handles as an argument | `Vec<Box<dyn Calculator<T>>>`, `Calculator<T>[]` | `05-rust-handle-collection` | build-red: no list literal in a call argument; declared with a fixed arity of two (blocked first by 3) |
| 6 | a nested call in `call:` | `WithPrecision(4)` inside `FromFormula` | `06-nested-call` | frontend-red: the argument grammar has no call form ("expected ')' to close call arguments") |
| 7 | a struct literal in `call:` | `FormulaOptions { precision: 4 }` | `07-rust-struct-literal` | build-red: accepted by the frontend; Rust cannot be told the foreign struct's own name nor wrap the value in `Some` (blocked first by 3) |
| 8 | construction by `new`, not a function call | TypeScript classes | `08-ts-new-construction` | build-red: no `new` form; "Value of type 'typeof ConstantCalculator' is not callable" |
| 9 | a method synchronous in one target, asynchronous in the others | `compute()` in TS | `09-ts-sync-method` | build-red: blocked by 8 (the constructor is a class); the `sync` marker itself is accepted |
| 10 | a handle composed and read separately | fallback + a read of one of its inputs | `10-ownership-refused` | refused, as intended: single ownership is the rule, and the generator names the field and both readers |

Passing today: capability 10 (the one that is a rule, not a gap). Zero of
the nine gaps pass on any target.

### The bench proper (`service.tono`)

The full three-target file compiles through the frontend and stops at
generation on every target, before any capability above is reached, because
the declared-test emitters do not carry what the bench needs:

- Go (`bench go`, gen-red): a `[]float @arg` pinned in a test renders as
  `[]float64("")`, and four stubs of the same handle type in one test emit
  the same fake type four times.
- TypeScript and Rust (`bench typescript`, `bench rust`, gen-red): the
  emitter panics on `stub mathkit.calculator.compute` (a handle-method stub;
  only Go renders one).

These are not among the ten capabilities; they are the first thing the
bench found. Once they clear, the same file will stop at the capabilities
the probes already report.

## Updating the record

When an emitter change makes a check end somewhere else than `gate.tsv`
says, the gate fails and names the row. Progress is recorded by moving that
row's expected outcome forward and updating the table above; a check that
reaches `build` needs its driver next to it (`probes/<id>.verify.{go,rs,test.ts}`,
or `verify/` for the bench proper) or the gate reports `no-driver`. Never
make a check pass by changing the stand-in library to fit the emitter: the
library's shape is the point.
