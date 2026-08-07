(* JSON codec for the resolved wire binding. External callers go through
   [Ir_json], which folds this into the operation shape kind. *)

val encode_wire_response_part : Ir.wire_response_part -> Ir.json

val decode_wire_response_part :
  Ir.json -> (Ir.wire_response_part, string) result

val encode_wire_value : Ir.wire_value -> Ir.json
val decode_wire_value : Ir.json -> (Ir.wire_value, string) result
val encode_wire_binding : Ir.wire_binding -> Ir.json
val decode_wire_binding : Ir.json -> (Ir.wire_binding, string) result

val decode_wire_binding_opt :
  Ir.json option -> (Ir.wire_binding option, string) result
