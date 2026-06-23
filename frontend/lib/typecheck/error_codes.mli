(* Stable typecheck diagnostic codes. Each maps to one well-defined failure so
   tooling and tests can pin behaviour by code rather than message text. *)

val unknown_type : string
val duplicate_shape : string

(* TC0003 is intentionally unused: type-parameter scope is resolved during
   lowering, so an out-of-scope name surfaces as [unknown_type]. *)
val generic_arity_mismatch : string
val non_generic_applied : string
