(* JSON wire encoding for the IR -- the contract the Rust backend mirrors.
   Encoders assume well-formed in-memory values and raise [Ir.Invalid_ir] on the
   few things JSON cannot represent (non-finite floats, out-of-set integer
   widths, a [Custom] constraint in the structured field). Decoders take
   untrusted input and return a [result]. *)

(* The IR schema revision this build understands; a decoder rejects any other. *)
val current_ir_version : int
val encode_prim : Ir.prim -> Ir.json
val decode_prim : Ir.json -> (Ir.prim, string) result
val encode_tref : Ir.tref -> Ir.json
val decode_tref : Ir.json -> (Ir.tref, string) result
val encode_constraint : Ir.constraint_ -> Ir.json
val decode_constraint : Ir.json -> (Ir.constraint_, string) result
val encode_trait : Ir.trait -> Ir.json
val decode_trait : Ir.json -> (Ir.trait, string) result
val encode_member : Ir.member -> Ir.json
val decode_member : Ir.json -> (Ir.member, string) result
val encode_shape : Ir.shape -> Ir.json
val decode_shape : Ir.json -> (Ir.shape, string) result
val encode_module : Ir.module_ -> Ir.json
val decode_module : Ir.json -> (Ir.module_, string) result
val encode_model : Ir.model -> Ir.json

(* Decode a model, rejecting a [tono_ir_version] this build does not recognize
   before decoding the rest. *)
val decode_model : Ir.json -> (Ir.model, string) result

(* Canonical string form (object keys sorted, large ints collapsed when they fit
   a native int) for stable comparison across emitters. *)
val to_canonical_string : Ir.json -> string

(* Low-level coercion helpers, exposed only so their edge cases (e.g. [`Intlit]
   that fits or overflows a native int) can be tested directly. Not part of the
   intended public surface. *)
module Internal : sig
  val as_int : Ir.json -> (int, string) result
  val as_float : Ir.json -> (float, string) result
end
