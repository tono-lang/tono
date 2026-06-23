(* Stable calculus diagnostic codes. Separate from the typecheck [TCxxxx]
   catalogue: the calculus is its own checker. [type_mismatch] is the catch-all
   for incompatible operands, arguments, branches, and coalesce arms; the message
   says precisely what was expected. *)

let unbound = "CA0001" (* a variable or function name is not in scope *)

let wrong_arity =
  "CA0002" (* a builtin or function got the wrong argument count *)

let type_mismatch = "CA0003" (* an operand/argument/branch type does not fit *)
let field_access = "CA0004" (* '.' on a non-struct, or an unknown field *)

let unguarded_division =
  "CA0005" (* '/' or '%' without a proof the divisor != 0 *)

let match_subject = "CA0006" (* match on something that is not a sum type *)
let unknown_variant = "CA0007" (* a match arm names a variant the sum lacks *)

let non_exhaustive =
  "CA0008" (* a variant is unmatched, or the Unknown arm is missing *)

let divergent_arms = "CA0009" (* match arms produce different types *)
let recursive_fn = "CA0010" (* the function call graph has a cycle *)
