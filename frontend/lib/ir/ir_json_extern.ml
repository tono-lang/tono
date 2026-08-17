(* JSON codecs for FFI library declarations (module [ext_libs]): per-language
   module paths, foreign struct/opaque-handle declarations, and [extern]
   declarations with their per-language call/yields/returns/errors bindings.
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

let encode_foreign_field (f : Ir.foreign_field) : Ir.json =
  `Assoc [ ("name", `String f.fgf_name); ("type", encode_tref f.fgf_type) ]

let encode_foreign_struct (s : Ir.foreign_struct) : Ir.json =
  `Assoc
    [
      ("name", `String s.fgs_name);
      ("fields", `List (List.map encode_foreign_field s.fgs_fields));
    ]

let encode_yields_pos (y : Ir.yields_pos) : Ir.json =
  `Assoc
    ([ ("name", `String y.yp_name) ]
    @ (match y.yp_type with
      | None -> []
      | Some t -> [ ("type", encode_tref t) ])
    @ if y.yp_is_error then [ ("is_error", `Bool true) ] else [])

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

let encode_error_binding (e : Ir.error_binding) : Ir.json =
  `Assoc [ ("sentinel", `String e.erb_sentinel); ("type", `String e.erb_type) ]

let encode_extern_lang (l : Ir.extern_lang) : Ir.json =
  `Assoc
    ([
       ("lang", `String l.el_lang);
       ("symbol", `String l.el_symbol);
       ("call_args", `List (List.map encode_call_arg l.el_call_args));
     ]
    @ (if l.el_yields = [] then []
       else [ ("yields", `List (List.map encode_yields_pos l.el_yields)) ])
    @ (match l.el_returns with
      | None -> []
      | Some r -> [ ("returns", encode_returns_lit r) ])
    @ (if l.el_errors = [] then []
       else [ ("errors", `List (List.map encode_error_binding l.el_errors)) ])
    @ (if l.el_sync then [ ("sync", `Bool true) ] else [])
    @ (if l.el_infallible then [ ("infallible", `Bool true) ] else [])
    @ if l.el_ctx then [ ("ctx", `Bool true) ] else [])

let encode_extern_param (p : Ir.extern_param) : Ir.json =
  `Assoc [ ("name", `String p.xp_name); ("type", encode_tref p.xp_type) ]

let encode_extern_decl (e : Ir.extern_decl) : Ir.json =
  `Assoc
    [
      ("name", `String e.x_name);
      ("params", `List (List.map encode_extern_param e.x_params));
      ("return", encode_tref e.x_return);
      ("langs", `List (List.map encode_extern_lang e.x_langs));
    ]

let encode_opaque_instance (i : Ir.opaque_instance) : Ir.json =
  `Assoc
    [
      ("foreign_name", `String i.inst_foreign_name);
      ("arg", encode_tref i.inst_arg);
    ]

let encode_opaque_type (t : Ir.opaque_type) : Ir.json =
  `Assoc
    ([
       ("name", `String t.opq_name);
       ("methods", `List (List.map encode_extern_decl t.opq_methods));
     ]
    @
    match t.opq_instance with
    | None -> []
    | Some i -> [ ("instance", encode_opaque_instance i) ])

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

let decode_lang_path j =
  let* kvs = as_assoc j in
  let* lang =
    match List.assoc_opt "lang" kvs with
    | Some v -> as_string v
    | None -> err "lang path is missing lang"
  in
  let* path =
    match List.assoc_opt "path" kvs with
    | Some v -> as_string v
    | None -> err "lang path is missing path"
  in
  Ok ({ Ir.lgp_lang = lang; lgp_path = path } : Ir.lang_path)

let decode_foreign_field j =
  let* kvs = as_assoc j in
  let* name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "foreign field is missing name"
  in
  let* ty =
    match List.assoc_opt "type" kvs with
    | Some v -> decode_tref v
    | None -> err "foreign field is missing type"
  in
  Ok ({ Ir.fgf_name = name; fgf_type = ty } : Ir.foreign_field)

let decode_foreign_struct j =
  let* kvs = as_assoc j in
  let* name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "foreign struct is missing name"
  in
  let* fields =
    match List.assoc_opt "fields" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_foreign_field xs
  in
  Ok ({ Ir.fgs_name = name; fgs_fields = fields } : Ir.foreign_struct)

let decode_yields_pos j =
  let* kvs = as_assoc j in
  let* name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "yields position is missing name"
  in
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
    | Some v -> Ir_json_base.as_bool v
  in
  Ok
    ({ Ir.yp_name = name; yp_type = ty; yp_is_error = is_error }
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
  let* name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "returns field is missing name"
  in
  let* value =
    match List.assoc_opt "value" kvs with
    | Some v -> decode_returns_value v
    | None -> err "returns field is missing value"
  in
  Ok ({ Ir.rvf_name = name; rvf_value = value } : Ir.returns_field)

let decode_returns_lit j =
  let* kvs = as_assoc j in
  let* ty =
    match List.assoc_opt "type" kvs with
    | Some v -> decode_tref v
    | None -> err "returns is missing type"
  in
  let* fields =
    match List.assoc_opt "fields" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_returns_field xs
  in
  Ok ({ Ir.rvl_type = ty; rvl_fields = fields } : Ir.returns_lit)

let decode_error_binding j =
  let* kvs = as_assoc j in
  let* sentinel =
    match List.assoc_opt "sentinel" kvs with
    | Some v -> as_string v
    | None -> err "error binding is missing sentinel"
  in
  let* ty =
    match List.assoc_opt "type" kvs with
    | Some v -> as_string v
    | None -> err "error binding is missing type"
  in
  Ok ({ Ir.erb_sentinel = sentinel; erb_type = ty } : Ir.error_binding)

let decode_extern_lang j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* lang =
    match get "lang" with
    | Some v -> as_string v
    | None -> err "language block is missing lang"
  in
  let* symbol =
    match get "symbol" with
    | Some v -> as_string v
    | None -> err "language block is missing symbol"
  in
  let* call_args =
    match get "call_args" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_call_arg xs
  in
  let* yields =
    match get "yields" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_yields_pos xs
  in
  let* returns =
    match get "returns" with
    | None -> Ok None
    | Some v ->
        let* r = decode_returns_lit v in
        Ok (Some r)
  in
  let* errors =
    match get "errors" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_error_binding xs
  in
  let* sync = match get "sync" with None -> Ok false | Some v -> as_bool v in
  let* infallible =
    match get "infallible" with None -> Ok false | Some v -> as_bool v
  in
  let* ctx = match get "ctx" with None -> Ok false | Some v -> as_bool v in
  Ok
    ({
       el_lang = lang;
       el_symbol = symbol;
       el_call_args = call_args;
       el_yields = yields;
       el_returns = returns;
       el_errors = errors;
       el_sync = sync;
       el_infallible = infallible;
       el_ctx = ctx;
     }
      : Ir.extern_lang)

let decode_extern_param j =
  let* kvs = as_assoc j in
  let* name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "extern param is missing name"
  in
  let* ty =
    match List.assoc_opt "type" kvs with
    | Some v -> decode_tref v
    | None -> err "extern param is missing type"
  in
  Ok ({ Ir.xp_name = name; xp_type = ty } : Ir.extern_param)

let rec decode_extern_decl j =
  let* kvs = as_assoc j in
  let* name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "extern is missing name"
  in
  let* params =
    match List.assoc_opt "params" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_extern_param xs
  in
  let* ret =
    match List.assoc_opt "return" kvs with
    | Some v -> decode_tref v
    | None -> err "extern is missing return"
  in
  let* langs =
    match List.assoc_opt "langs" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_extern_lang xs
  in
  Ok
    ({ Ir.x_name = name; x_params = params; x_return = ret; x_langs = langs }
      : Ir.extern_decl)

and decode_opaque_instance j =
  let* kvs = as_assoc j in
  let* foreign_name =
    match List.assoc_opt "foreign_name" kvs with
    | Some v -> as_string v
    | None -> err "opaque instance is missing foreign_name"
  in
  let* arg =
    match List.assoc_opt "arg" kvs with
    | Some v -> decode_tref v
    | None -> err "opaque instance is missing arg"
  in
  Ok
    ({ Ir.inst_foreign_name = foreign_name; inst_arg = arg }
      : Ir.opaque_instance)

and decode_opaque_type j =
  let* kvs = as_assoc j in
  let* name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "opaque type is missing name"
  in
  let* instance =
    match List.assoc_opt "instance" kvs with
    | None -> Ok None
    | Some v ->
        let* i = decode_opaque_instance v in
        Ok (Some i)
  in
  let* methods =
    match List.assoc_opt "methods" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_extern_decl xs
  in
  Ok
    ({ Ir.opq_name = name; opq_instance = instance; opq_methods = methods }
      : Ir.opaque_type)

let decode_ext_lib j =
  let* kvs = as_assoc j in
  let* name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "ext lib is missing name"
  in
  let list_of k dec =
    match List.assoc_opt k kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result dec xs
  in
  let* langs = list_of "langs" decode_lang_path in
  let* structs = list_of "structs" decode_foreign_struct in
  let* types = list_of "types" decode_opaque_type in
  let* externs = list_of "externs" decode_extern_decl in
  Ok
    ({
       Ir.xl_name = name;
       xl_langs = langs;
       xl_structs = structs;
       xl_types = types;
       xl_externs = externs;
     }
      : Ir.ext_lib)
