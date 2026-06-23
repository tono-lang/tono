(* Stable typecheck diagnostic codes. Each maps to one well-defined failure so
   tooling and tests can pin behaviour by code rather than message text. *)

let unknown_type = "TC0001"
let duplicate_shape = "TC0002"

(* TC0003 is intentionally unused: type-parameter scope is resolved during
   lowering, so an out-of-scope name surfaces as [unknown_type] rather than a
   distinct unbound-parameter code. *)
let generic_arity_mismatch = "TC0004"
let non_generic_applied = "TC0005"
