(* Stable typecheck diagnostic codes. Each maps to one well-defined failure so
   tooling and tests can pin behaviour by code rather than message text. *)

let unknown_type = "TC0001"
let duplicate_shape = "TC0002"

(* TC0003 is intentionally unused: type-parameter scope is resolved during
   lowering, so an out-of-scope name surfaces as [unknown_type] rather than a
   distinct unbound-parameter code. *)
let generic_arity_mismatch = "TC0004"
let non_generic_applied = "TC0005"

(* TC0006 (bounds_not_supported) is rejected at parse time; the checker never
   reaches a bound to report, so the code has no constant here. *)
let nullability_conflict = "TC0007"
let enum_value_duplicate = "TC0008"
let enum_backing_mismatch = "TC0009"
let constraint_type_mismatch = "TC0010"
let constraint_malformed = "TC0011"
let default_type_mismatch = "TC0012"
let default_violates_constraint = "TC0013"
let unresolved_operation_ref = "TC0014"
let error_status_missing = "TC0015"
let error_code_invalid = "TC0016"
let error_discrimination_ambiguous = "TC0017"
let async_takes_no_arguments = "TC0018"
