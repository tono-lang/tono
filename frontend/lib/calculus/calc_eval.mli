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

(* A runtime value. Integers carry their width and signedness so arithmetic
   wraps correctly. *)
type value =
  | VInt of int64 * int * bool
  | VFloat of float
  | VStr of string
  | VBool of bool
  | VList of value list
  | VMap of (value * value) list
  | VStruct of (string * value) list
  | VVariant of string * value
  | VOpt of value option

(* Raised only on an ill-typed program (which the type checker rules out); a
   well-typed program evaluates totally without reaching it. *)
exception Stuck of string

(* An i64 value from an OCaml int, for building test inputs. *)
val vint : int -> value

(* Evaluate a named entry function against already-evaluated argument values.
   Total for a well-typed program: it terminates and returns a value. *)
val eval_fn : Calc_ast.program -> string -> value list -> value
