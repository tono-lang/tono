(* JSON codecs for the declared-test surface. [Ir_json] composes these into the
   module encoding; external callers go through [Ir_json]. *)

val encode_test : Ir.test_decl -> Ir.json
val decode_test : Ir.json -> (Ir.test_decl, string) result
val encode_pattern : Ir.test_pattern -> Ir.json
val decode_pattern : Ir.json -> (Ir.test_pattern, string) result
val encode_request_pattern : Ir.request_pattern -> Ir.json
val decode_request_pattern : Ir.json -> (Ir.request_pattern, string) result
