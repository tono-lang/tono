(* Stable typecheck diagnostic codes. Each maps to one well-defined failure so
   tooling and tests can pin behaviour by code rather than message text. *)

val unknown_type : string
val duplicate_shape : string

(* TC0003 is intentionally unused: type-parameter scope is resolved during
   lowering, so an out-of-scope name surfaces as [unknown_type]. *)
val generic_arity_mismatch : string
val non_generic_applied : string

(* TC0006 (bounds_not_supported) is a parse-level rejection; no constant here. *)
val nullability_conflict : string
val enum_value_duplicate : string
val enum_backing_mismatch : string
val constraint_type_mismatch : string
val constraint_malformed : string
val default_type_mismatch : string
val default_violates_constraint : string
