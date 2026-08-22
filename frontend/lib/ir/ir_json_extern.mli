(* JSON codecs for FFI library declarations (module [ext_libs]). External
   callers go through [Ir_json], which folds these into the module envelope. *)

val encode_lang_path : Ir.lang_path -> Ir.json
val decode_lang_path : Ir.json -> (Ir.lang_path, string) result
val encode_foreign_struct : Ir.foreign_struct -> Ir.json
val decode_foreign_struct : Ir.json -> (Ir.foreign_struct, string) result
val encode_opaque_type : Ir.opaque_type -> Ir.json
val decode_opaque_type : Ir.json -> (Ir.opaque_type, string) result
val encode_extern_decl : Ir.extern_decl -> Ir.json
val decode_extern_decl : Ir.json -> (Ir.extern_decl, string) result
val encode_ext_lib : Ir.ext_lib -> Ir.json
val decode_ext_lib : Ir.json -> (Ir.ext_lib, string) result
val encode_foreign_lang : Ir.foreign_lang -> Ir.json
val decode_foreign_lang : Ir.json -> (Ir.foreign_lang, string) result
