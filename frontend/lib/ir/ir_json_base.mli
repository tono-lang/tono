(* Scalar-level JSON codecs for the IR (primitives, type references, core
   constraints, traits, members, enum values) plus the shared untrusted-input
   coercion helpers. [Ir_json] composes these into shapes, modules, and the
   versioned model envelope; external callers go through [Ir_json]. *)

val encode_prim : Ir.prim -> Ir.json
val decode_prim : Ir.json -> (Ir.prim, string) result
val encode_tref : Ir.tref -> Ir.json
val decode_tref : Ir.json -> (Ir.tref, string) result
val decode_tref_opt : Ir.json option -> (Ir.tref option, string) result
val encode_constraint : Ir.constraint_ -> Ir.json
val decode_constraint : Ir.json -> (Ir.constraint_, string) result
val encode_trait : Ir.trait -> Ir.json
val decode_trait : Ir.json -> (Ir.trait, string) result
val encode_member : Ir.member -> Ir.json
val decode_member : Ir.json -> (Ir.member, string) result
val encode_enum_value : Ir.enum_value -> Ir.json
val decode_enum_value : Ir.json -> (Ir.enum_value, string) result
val encode_backing : [ `String | `Int ] -> string

(* Decoder plumbing shared by [Ir_json] and [Ir_json_entry]. *)
val map_result :
  ('a -> ('b, string) result) -> 'a list -> ('b list, string) result

val as_assoc : Ir.json -> ((string * Ir.json) list, string) result
val as_list : Ir.json -> (Ir.json list, string) result
val as_string : Ir.json -> (string, string) result
val as_bool : Ir.json -> (bool, string) result
val as_int : Ir.json -> (int, string) result
val as_float : Ir.json -> (float, string) result

val ensure_only :
  string list -> (string * Ir.json) list -> (unit, string) result
