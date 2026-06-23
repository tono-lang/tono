(* The reference evaluator: the calculus's own evaluation is the semantic
   reference the five target languages must reproduce bit-for-bit. It is total --
   it never throws and always returns a value -- which is exactly the property a
   well-typed program guarantees.

   The load-bearing part is the deterministic integer semantics: arithmetic wraps
   (two's complement at the type's width for signed, modulo 2^w for unsigned) and
   division/modulo truncate toward zero with the remainder taking the sign of the
   dividend. Values are normalized to their width on every operation. *)

(* ── Numeric semantics (the cross-language contract, in OCaml) ─────────── *)

module Num = struct
  (* Normalize a 64-bit pattern to [bits] width, sign-extending when signed. *)
  let wrap ~bits ~signed (v : int64) : int64 =
    if bits >= 64 then v
    else
      let mask = Int64.sub (Int64.shift_left 1L bits) 1L in
      let m = Int64.logand v mask in
      if signed && Int64.logand m (Int64.shift_left 1L (bits - 1)) <> 0L then
        Int64.logor m (Int64.lognot mask) (* sign-extend *)
      else m

  let add ~bits ~signed a b = wrap ~bits ~signed (Int64.add a b)
  let sub ~bits ~signed a b = wrap ~bits ~signed (Int64.sub a b)
  let mul ~bits ~signed a b = wrap ~bits ~signed (Int64.mul a b)

  (* Truncated division/modulo. [INT_MIN / -1] is defined to wrap rather than
     trap, so the only obligation the type checker imposes is a non-zero divisor;
     a zero divisor cannot reach here in a well-typed program, so it is treated
     defensively as a no-op. *)
  let div ~bits ~signed a b =
    if Int64.equal b 0L then 0L
    else if signed then wrap ~bits ~signed (Int64.div a b)
    else wrap ~bits ~signed (Int64.unsigned_div a b)

  let rem ~bits ~signed a b =
    if Int64.equal b 0L then 0L
    else if signed then wrap ~bits ~signed (Int64.rem a b)
    else wrap ~bits ~signed (Int64.unsigned_rem a b)

  (* float -> int? : truncate toward zero; non-finite has no integer form. *)
  let to_int (f : float) : int64 option =
    if Float.is_finite f then Some (Int64.of_float (Float.trunc f)) else None

  (* int -> float : total, lossy above 2^53. *)
  let to_float ~signed (v : int64) : float =
    if signed then Int64.to_float v
    else if Int64.compare v 0L >= 0 then Int64.to_float v
    else
      (* interpret the bit pattern as unsigned *)
      (Int64.to_float (Int64.shift_right_logical v 1) *. 2.0)
      +. Int64.to_float (Int64.logand v 1L)
end
