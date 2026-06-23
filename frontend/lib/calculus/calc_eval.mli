(* The reference numeric semantics the five target languages must reproduce
   bit-for-bit: wrapping integer arithmetic, truncated division/modulo with the
   remainder taking the sign of the dividend, and total float<->int coercions.
   Values are normalized to their width on every operation; nothing throws. *)

module Num : sig
  (* Normalize a 64-bit pattern to [bits] width (sign-extending when [signed]). *)
  val wrap : bits:int -> signed:bool -> int64 -> int64
  val add : bits:int -> signed:bool -> int64 -> int64 -> int64
  val sub : bits:int -> signed:bool -> int64 -> int64 -> int64
  val mul : bits:int -> signed:bool -> int64 -> int64 -> int64

  (* Truncated toward zero; [INT_MIN / -1] wraps, a zero divisor yields 0. *)
  val div : bits:int -> signed:bool -> int64 -> int64 -> int64
  val rem : bits:int -> signed:bool -> int64 -> int64 -> int64

  (* Truncate toward zero; non-finite floats have no integer form. *)
  val to_int : float -> int64 option

  (* Total, lossy above 2^53. *)
  val to_float : signed:bool -> int64 -> float
end
