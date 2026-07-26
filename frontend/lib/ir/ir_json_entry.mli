(* JSON codecs for the entry-model surface: value sources, templates, match
   selection, binds, and entry fields. External callers go through [Ir_json],
   which folds these into the entry/config shape kinds. *)

val encode_source : Ir.source -> Ir.json
val decode_source : Ir.json -> (Ir.source, string) result
val encode_template_part : Ir.template_part -> Ir.json
val decode_template_part : Ir.json -> (Ir.template_part, string) result
val encode_arm_value : Ir.arm_value -> Ir.json
val decode_arm_value : Ir.json -> (Ir.arm_value, string) result
val encode_select : Ir.select -> Ir.json
val decode_select : Ir.json -> (Ir.select, string) result
val encode_bind : Ir.bind -> Ir.json
val decode_bind : Ir.json -> (Ir.bind, string) result
val encode_entry_field : Ir.entry_field -> Ir.json
val decode_entry_field : Ir.json -> (Ir.entry_field, string) result
