(* Stable calculus diagnostic codes (CAxxxx), separate from the typecheck
   catalogue. [type_mismatch] is the catch-all for incompatible types; the
   message specifies what was expected. *)

val unbound : string
val wrong_arity : string
val type_mismatch : string
val field_access : string
val unguarded_division : string
val match_subject : string
val unknown_variant : string
val non_exhaustive : string
val divergent_arms : string
val recursive_fn : string
