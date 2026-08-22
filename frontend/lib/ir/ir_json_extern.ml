(* JSON codecs for FFI library declarations (module [ext_libs]): per-language
   module paths, foreign struct/opaque-handle declarations, and ext op
   declarations with their per-language call/yields/returns bindings.
   Scalar and entry-model codecs come from [Ir_json_base]/[Ir_json_entry];
   [Ir_json] folds these into the module envelope. *)

let encode_tref = Ir_json_base.encode_tref
let decode_tref = Ir_json_base.decode_tref
let encode_select = Ir_json_entry.encode_select
let decode_select = Ir_json_entry.decode_select
let encode_call_arg = Ir_json_entry.encode_call_arg
let decode_call_arg = Ir_json_entry.decode_call_arg
let ( let* ) = Result.bind
let err fmt = Printf.ksprintf (fun s -> Error s) fmt
let map_result = Ir_json_base.map_result
let as_assoc = Ir_json_base.as_assoc
let as_list = Ir_json_base.as_list
let as_string = Ir_json_base.as_string
let as_bool = Ir_json_base.as_bool

(* ── Encoding ──────────────────────────────────────────────────────────── *)

let encode_lang_path (lp : Ir.lang_path) : Ir.json =
  `Assoc [ ("lang", `String lp.lgp_lang); ("path", `String lp.lgp_path) ]

let encode_foreign_lang (l : Ir.foreign_lang) : Ir.json =
  `Assoc
    ([ ("lang", `String l.fl_lang); ("name", `String l.fl_head) ]
    @
    if l.fl_fields = [] then []
    else
      [
        ( "fields",
          `Assoc (List.map (fun (n, sp) -> (n, `String sp)) l.fl_fields) );
      ])

let encode_foreign_field (f : Ir.foreign_field) : Ir.json =
  `Assoc [ ("name", `String f.fgf_name); ("type", encode_tref f.fgf_type) ]

let encode_foreign_struct (s : Ir.foreign_struct) : Ir.json =
  `Assoc
    [
      ("name", `String s.fgs_name);
      ("fields", `List (List.map encode_foreign_field s.fgs_fields));
      ("langs", `List (List.map encode_foreign_lang s.fgs_langs));
    ]

let encode_yields_pos (y : Ir.yields_pos) : Ir.json =
  `Assoc
    ([ ("name", `String y.yp_name) ]
    @ (match y.yp_type with
      | None -> []
      | Some t -> [ ("type", encode_tref t) ])
    @ (if y.yp_is_error then [ ("is_error", `Bool true) ] else [])
    @
    match y.yp_foreign with
    | None -> []
    | Some s -> [ ("foreign", `String s) ])

let encode_returns_value (v : Ir.returns_value) : Ir.json =
  match v with
  | Ir.Rv_ref p -> `Assoc [ ("field", `List (List.map (fun s -> `String s) p)) ]
  | Ir.Rv_select s -> `Assoc [ ("select", encode_select s) ]

let encode_returns_field (f : Ir.returns_field) : Ir.json =
  `Assoc
    [
      ("name", `String f.rvf_name); ("value", encode_returns_value f.rvf_value);
    ]

let encode_returns_lit (r : Ir.returns_lit) : Ir.json =
  `Assoc
    [
      ("type", encode_tref r.rvl_type);
      ("fields", `List (List.map encode_returns_field r.rvl_fields));
    ]

let encode_extern_lang (l : Ir.extern_lang) : Ir.json =
  `Assoc
    ([
       ("lang", `String l.el_lang);
       ("symbol", `String l.el_symbol);
       ("call_args", `List (List.map encode_call_arg l.el_call_args));
     ]
    @ (if l.el_yields = [] then []
       else [ ("yields", `List (List.map encode_yields_pos l.el_yields)) ])
    @
    match l.el_returns with
    | None -> []
    | Some r -> [ ("returns", encode_returns_lit r) ])

let encode_extern_param (p : Ir.extern_param) : Ir.json =
  `Assoc [ ("name", `String p.xp_name); ("type", encode_tref p.xp_type) ]

let strings xs = `List (List.map (fun s -> `String s) xs)

let encode_extern_decl (e : Ir.extern_decl) : Ir.json =
  `Assoc
    ([
       ("name", `String e.x_name);
       ("params", `List (List.map encode_extern_param e.x_params));
       ("return", encode_tref e.x_return);
       ("langs", `List (List.map encode_extern_lang e.x_langs));
     ]
    @ (if e.x_async = [] then [] else [ ("async", strings e.x_async) ])
    @ if e.x_errors = [] then [] else [ ("errors", strings e.x_errors) ])

let encode_opaque_type (t : Ir.opaque_type) : Ir.json =
  `Assoc
    [
      ("name", `String t.opq_name);
      ("langs", `List (List.map encode_foreign_lang t.opq_langs));
      ("methods", `List (List.map encode_extern_decl t.opq_methods));
    ]

let encode_ext_lib (l : Ir.ext_lib) : Ir.json =
  `Assoc
    [
      ("name", `String l.xl_name);
      ("langs", `List (List.map encode_lang_path l.xl_langs));
      ("structs", `List (List.map encode_foreign_struct l.xl_structs));
      ("types", `List (List.map encode_opaque_type l.xl_types));
      ("externs", `List (List.map encode_extern_decl l.xl_externs));
    ]

(* ── Decoding ──────────────────────────────────────────────────────────── *)

let field kvs k what dec =
  match List.assoc_opt k kvs with
  | Some v -> dec v
  | None -> err "%s is missing %s" what k

let list_field kvs k dec =
  match List.assoc_opt k kvs with
  | None -> Ok []
  | Some v ->
      let* xs = as_list v in
      map_result dec xs

let decode_lang_path j =
  let* kvs = as_assoc j in
  let* lang = field kvs "lang" "lang path" as_string in
  let* path = field kvs "path" "lang path" as_string in
  Ok ({ Ir.lgp_lang = lang; lgp_path = path } : Ir.lang_path)

let decode_foreign_lang j =
  let* kvs = as_assoc j in
  let* lang = field kvs "lang" "foreign lang" as_string in
  let* head = field kvs "name" "foreign lang" as_string in
  let* fields =
    match List.assoc_opt "fields" kvs with
    | None -> Ok []
    | Some v ->
        let* pairs = as_assoc v in
        map_result
          (fun (n, sp) ->
            let* sp = as_string sp in
            Ok (n, sp))
          pairs
  in
  Ok
    ({ Ir.fl_lang = lang; fl_head = head; fl_fields = fields }
      : Ir.foreign_lang)

let decode_foreign_field j =
  let* kvs = as_assoc j in
  let* name = field kvs "name" "foreign field" as_string in
  let* ty = field kvs "type" "foreign field" decode_tref in
  Ok ({ Ir.fgf_name = name; fgf_type = ty } : Ir.foreign_field)

let decode_foreign_struct j =
  let* kvs = as_assoc j in
  let* name = field kvs "name" "foreign struct" as_string in
  let* fields = list_field kvs "fields" decode_foreign_field in
  let* langs = list_field kvs "langs" decode_foreign_lang in
  Ok
    ({ Ir.fgs_name = name; fgs_fields = fields; fgs_langs = langs }
      : Ir.foreign_struct)

let decode_yields_pos j =
  let* kvs = as_assoc j in
  let* name = field kvs "name" "yields position" as_string in
  let* ty =
    match List.assoc_opt "type" kvs with
    | None -> Ok None
    | Some v ->
        let* t = decode_tref v in
        Ok (Some t)
  in
  let* is_error =
    match List.assoc_opt "is_error" kvs with
    | None -> Ok false
    | Some v -> as_bool v
  in
  let* foreign =
    match List.assoc_opt "foreign" kvs with
    | None -> Ok None
    | Some v ->
        let* s = as_string v in
        Ok (Some s)
  in
  Ok
    ({
       Ir.yp_name = name;
       yp_type = ty;
       yp_is_error = is_error;
       yp_foreign = foreign;
     }
      : Ir.yields_pos)

let decode_returns_value j =
  let* kvs = as_assoc j in
  match kvs with
  | [ ("field", v) ] ->
      let* xs = as_list v in
      let* p = map_result as_string xs in
      Ok (Ir.Rv_ref p)
  | [ ("select", v) ] ->
      let* s = decode_select v in
      Ok (Ir.Rv_select s)
  | _ -> err "returns value must be a single field or select key"

let decode_returns_field j =
  let* kvs = as_assoc j in
  let* name = field kvs "name" "returns field" as_string in
  let* value = field kvs "value" "returns field" decode_returns_value in
  Ok ({ Ir.rvf_name = name; rvf_value = value } : Ir.returns_field)

let decode_returns_lit j =
  let* kvs = as_assoc j in
  let* ty = field kvs "type" "returns" decode_tref in
  let* fields = list_field kvs "fields" decode_returns_field in
  Ok ({ Ir.rvl_type = ty; rvl_fields = fields } : Ir.returns_lit)

let decode_extern_lang j =
  let* kvs = as_assoc j in
  let* lang = field kvs "lang" "language block" as_string in
  let* symbol = field kvs "symbol" "language block" as_string in
  let* call_args = list_field kvs "call_args" decode_call_arg in
  let* yields = list_field kvs "yields" decode_yields_pos in
  let* returns =
    match List.assoc_opt "returns" kvs with
    | None -> Ok None
    | Some v ->
        let* r = decode_returns_lit v in
        Ok (Some r)
  in
  Ok
    ({
       el_lang = lang;
       el_symbol = symbol;
       el_call_args = call_args;
       el_yields = yields;
       el_returns = returns;
     }
      : Ir.extern_lang)

let decode_extern_param j =
  let* kvs = as_assoc j in
  let* name = field kvs "name" "extern param" as_string in
  let* ty = field kvs "type" "extern param" decode_tref in
  Ok ({ Ir.xp_name = name; xp_type = ty } : Ir.extern_param)

let decode_extern_decl j =
  let* kvs = as_assoc j in
  let* name = field kvs "name" "extern" as_string in
  let* params = list_field kvs "params" decode_extern_param in
  let* ret = field kvs "return" "extern" decode_tref in
  let* langs = list_field kvs "langs" decode_extern_lang in
  let* async = list_field kvs "async" as_string in
  let* errors = list_field kvs "errors" as_string in
  Ok
    ({
       Ir.x_name = name;
       x_params = params;
       x_return = ret;
       x_langs = langs;
       x_async = async;
       x_errors = errors;
     }
      : Ir.extern_decl)

let decode_opaque_type j =
  let* kvs = as_assoc j in
  let* name = field kvs "name" "opaque type" as_string in
  let* langs = list_field kvs "langs" decode_foreign_lang in
  let* methods = list_field kvs "methods" decode_extern_decl in
  Ok
    ({ Ir.opq_name = name; opq_langs = langs; opq_methods = methods }
      : Ir.opaque_type)

let decode_ext_lib j =
  let* kvs = as_assoc j in
  let* name = field kvs "name" "ext lib" as_string in
  let* langs = list_field kvs "langs" decode_lang_path in
  let* structs = list_field kvs "structs" decode_foreign_struct in
  let* types = list_field kvs "types" decode_opaque_type in
  let* externs = list_field kvs "externs" decode_extern_decl in
  Ok
    ({
       Ir.xl_name = name;
       xl_langs = langs;
       xl_structs = structs;
       xl_types = types;
       xl_externs = externs;
     }
      : Ir.ext_lib)
