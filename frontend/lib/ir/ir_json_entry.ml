(* JSON codecs for the entry-model surface: value sources, templates, match
   selection, binds, and entry fields. Scalar codecs come from
   [Ir_json_base]; [Ir_json] folds these into the entry/config shape kinds. *)

let encode_tref = Ir_json_base.encode_tref
let encode_constraint = Ir_json_base.encode_constraint
let encode_trait = Ir_json_base.encode_trait
let decode_tref = Ir_json_base.decode_tref
let decode_constraint = Ir_json_base.decode_constraint
let decode_trait = Ir_json_base.decode_trait
let ( let* ) = Result.bind
let err fmt = Printf.ksprintf (fun s -> Error s) fmt
let map_result = Ir_json_base.map_result
let as_assoc = Ir_json_base.as_assoc
let as_list = Ir_json_base.as_list
let as_string = Ir_json_base.as_string
let ensure_only = Ir_json_base.ensure_only

(* ── Encoding ──────────────────────────────────────────────────────────── *)

let encode_path (segs : string list) : Ir.json =
  `List (List.map (fun s -> `String s) segs)

let encode_source (s : Ir.source) : Ir.json =
  match s with
  | Ir.Arg -> `String "arg"
  | Ir.With -> `String "with"
  | Ir.Env (Ir.Env_name n) -> `Assoc [ ("env", `String n) ]
  | Ir.Env (Ir.Env_field p) ->
      `Assoc [ ("env", `Assoc [ ("field", encode_path p) ]) ]
  | Ir.Default v -> `Assoc [ ("default", v) ]

let encode_template_part (p : Ir.template_part) : Ir.json =
  match p with
  | Ir.Tpl_lit s -> `Assoc [ ("lit", `String s) ]
  | Ir.Tpl_field path -> `Assoc [ ("field", encode_path path) ]
  | Ir.Tpl_param path -> `Assoc [ ("param", encode_path path) ]
  | Ir.Tpl_input name -> `Assoc [ ("input", `String name) ]

let encode_arm_value (v : Ir.arm_value) : Ir.json =
  match v with
  | Ir.Arm_field path -> `Assoc [ ("field", encode_path path) ]
  | Ir.Arm_lit j -> `Assoc [ ("lit", j) ]
  | Ir.Arm_sources ss ->
      `Assoc [ ("sources", `List (List.map encode_source ss)) ]
  (* The unit-variant shape Rust's externally-tagged serde default gives a
     payload-less enum case: a bare tag string, not an object. *)
  | Ir.Arm_subject -> `String "subject"

let encode_select (s : Ir.select) : Ir.json =
  let arm (a : Ir.select_arm) =
    `Assoc
      ((match a.arm_pattern with Some p -> [ ("pattern", p) ] | None -> [])
      @ [ ("value", encode_arm_value a.arm_value) ])
  in
  `Assoc
    ([
       ("subject", encode_path s.subject); ("arms", `List (List.map arm s.arms));
     ]
    @
    match s.subject_index with
    | None -> []
    | Some idx -> [ ("subject_index", encode_path idx) ])

let encode_bind (b : Ir.bind) : Ir.json =
  `Assoc
    [ ("field", `String b.bind_field); ("source", encode_path b.bind_source) ]

(* An extern call argument, shared by a field's own [= ns.fn(args)] source, an
   ext block's own [call:]/opaque-method bodies, and a ctor field's value
   (where [lit]/[list]/[call] can also occur, e.g. [opts { retries: 3 }]). *)
let rec encode_call_arg (a : Ir.call_arg) : Ir.json =
  match a with
  | Ir.Ca_param n -> `Assoc [ ("param", `String n) ]
  | Ir.Ca_ref p -> `Assoc [ ("field", encode_path p) ]
  | Ir.Ca_ctor c -> encode_call_ctor c
  | Ir.Ca_lit v -> `Assoc [ ("lit", v) ]
  | Ir.Ca_list xs -> `Assoc [ ("list", `List (List.map encode_call_arg xs)) ]
  | Ir.Ca_call c -> `Assoc [ ("call", encode_entry_call c) ]
  | Ir.Ca_symbol_call sc ->
      `Assoc
        [
          ("symbol", `String sc.scl_symbol);
          ("symbol_args", `List (List.map encode_call_arg sc.scl_args));
        ]
  | Ir.Ca_type n -> `Assoc [ ("type", `String n) ]
  (* Pairs, not an object: written order is part of the value (the emitted
     literal keeps it), and a JSON object does not promise one. *)
  | Ir.Ca_map entries ->
      `Assoc
        [
          ( "map",
            `List
              (List.map
                 (fun (k, v) -> `List [ `String k; encode_call_arg v ])
                 entries) );
        ]

and encode_call_ctor (c : Ir.call_ctor) : Ir.json =
  `Assoc
    [
      ("ctor", `String c.cc_name);
      ( "fields",
        `Assoc (List.map (fun (n, v) -> (n, encode_call_arg v)) c.cc_fields) );
    ]

and encode_entry_call (c : Ir.entry_call) : Ir.json =
  `Assoc
    [
      ("ns", `String c.ec_ns);
      ("fn", `String c.ec_fn);
      ("args", `List (List.map encode_call_arg c.ec_args));
    ]

(* A handle method call ([.field.method(args)]): an op's own "impl" body or
   a field's own value source. [recv] mirrors [entry_call]'s "ns"/"fn" shape,
   but as a field path rather than a bare "ext" namespace, since the receiver
   is an entry field. *)
let encode_op_impl_call (c : Ir.op_impl_call) : Ir.json =
  `Assoc
    [
      ("recv", `List (List.map (fun s -> `String s) c.Ir.oic_recv));
      ("method", `String c.Ir.oic_method);
      ("args", `List (List.map encode_call_arg c.Ir.oic_args));
    ]

let encode_entry_field (f : Ir.entry_field) : Ir.json =
  `Assoc
    ([
       ("name", `String f.ef_name);
       ("target", encode_tref f.ef_target);
       ("sources", `List (List.map encode_source f.ef_sources));
     ]
    @ (match f.ef_format with
      | None -> []
      | Some parts ->
          [ ("format", `List (List.map encode_template_part parts)) ])
    @ [ ("transforms", `List (List.map (fun t -> `String t) f.ef_transforms)) ]
    @ (match f.ef_select with
      | None -> []
      | Some s -> [ ("select", encode_select s) ])
    @ (match f.ef_call with
      | None -> []
      | Some c -> [ ("call", encode_entry_call c) ])
    @ (match f.ef_handle_call with
      | None -> []
      | Some c -> [ ("handle_call", encode_op_impl_call c) ])
    @ [
        ("binds", `List (List.map encode_bind f.ef_binds));
        ("constraints", `List (List.map encode_constraint f.ef_constraints));
        ("traits", `List (List.map encode_trait f.ef_traits));
      ])

(* ── Decoding ──────────────────────────────────────────────────────────── *)

let decode_path j =
  let* xs = as_list j in
  map_result as_string xs

let decode_source j =
  match j with
  | `String "arg" -> Ok Ir.Arg
  | `String "with" -> Ok Ir.With
  | `String other -> err "unknown source %S" other
  | `Assoc kvs -> (
      match kvs with
      | [ ("env", `String n) ] -> Ok (Ir.Env (Ir.Env_name n))
      | [ ("env", `Assoc [ ("field", p) ]) ] ->
          let* path = decode_path p in
          Ok (Ir.Env (Ir.Env_field path))
      | [ ("default", v) ] -> Ok (Ir.Default v)
      | _ -> err "source object must be a single env or default key")
  | _ -> err "expected a source"

let decode_template_part j =
  let* kvs = as_assoc j in
  match kvs with
  | [ ("lit", v) ] ->
      let* s = as_string v in
      Ok (Ir.Tpl_lit s)
  | [ ("field", v) ] ->
      let* p = decode_path v in
      Ok (Ir.Tpl_field p)
  | [ ("param", v) ] ->
      let* p = decode_path v in
      Ok (Ir.Tpl_param p)
  | [ ("input", v) ] ->
      let* s = as_string v in
      Ok (Ir.Tpl_input s)
  | _ -> err "template part must be a single lit, field, param, or input key"

let decode_arm_value j =
  match j with
  | `String "subject" -> Ok Ir.Arm_subject
  | `String other -> err "unknown arm value %S" other
  | _ -> (
      let* kvs = as_assoc j in
      match kvs with
      | [ ("field", v) ] ->
          let* p = decode_path v in
          Ok (Ir.Arm_field p)
      | [ ("lit", v) ] -> Ok (Ir.Arm_lit v)
      | [ ("sources", v) ] ->
          let* xs = as_list v in
          let* ss = map_result decode_source xs in
          Ok (Ir.Arm_sources ss)
      | _ -> err "arm value must be a single field, lit, or sources key")

let decode_select j =
  let* kvs = as_assoc j in
  let* subject =
    match List.assoc_opt "subject" kvs with
    | Some v -> decode_path v
    | None -> err "select is missing subject"
  in
  let* subject_index =
    match List.assoc_opt "subject_index" kvs with
    | None -> Ok None
    | Some v ->
        let* p = decode_path v in
        Ok (Some p)
  in
  let decode_arm aj =
    let* akvs = as_assoc aj in
    let* () = ensure_only [ "pattern"; "value" ] akvs in
    (* Absent means the wildcard arm; the mandatory "null" pattern of an
       optional subject is [Lower]'s {"null": true} marker, never a bare
       JSON null (see [Lower.lower_pattern] for why). *)
    let pattern = List.assoc_opt "pattern" akvs in
    let* value =
      match List.assoc_opt "value" akvs with
      | Some v -> decode_arm_value v
      | None -> err "select arm is missing value"
    in
    Ok ({ arm_pattern = pattern; arm_value = value } : Ir.select_arm)
  in
  let* arms =
    match List.assoc_opt "arms" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_arm xs
  in
  Ok ({ subject; subject_index; arms } : Ir.select)

let decode_bind j =
  let* kvs = as_assoc j in
  let* field =
    match List.assoc_opt "field" kvs with
    | Some v -> as_string v
    | None -> err "bind is missing field"
  in
  let* source =
    match List.assoc_opt "source" kvs with
    | Some v -> decode_path v
    | None -> err "bind is missing source"
  in
  Ok ({ bind_field = field; bind_source = source } : Ir.bind)

let rec decode_call_arg j =
  let* kvs = as_assoc j in
  match kvs with
  | [ ("param", v) ] ->
      let* s = as_string v in
      Ok (Ir.Ca_param s)
  | [ ("field", v) ] ->
      let* p = decode_path v in
      Ok (Ir.Ca_ref p)
  | [ ("lit", v) ] -> Ok (Ir.Ca_lit v)
  | [ ("type", v) ] ->
      let* s = as_string v in
      Ok (Ir.Ca_type s)
  | [ ("list", v) ] ->
      let* xs = as_list v in
      let* items = map_result decode_call_arg xs in
      Ok (Ir.Ca_list items)
  | [ ("call", v) ] ->
      let* c = decode_entry_call v in
      Ok (Ir.Ca_call c)
  | [ ("map", v) ] ->
      let* xs = as_list v in
      let* entries =
        map_result
          (fun e ->
            match e with
            | `List [ `String k; v ] ->
                let* a = decode_call_arg v in
                Ok (k, a)
            | _ -> err "map entry must be a [key, value] pair")
          xs
      in
      Ok (Ir.Ca_map entries)
  | ("symbol", _) :: _ ->
      let* symbol =
        match List.assoc_opt "symbol" kvs with
        | Some v -> as_string v
        | None -> err "symbol call is missing symbol"
      in
      let* args =
        match List.assoc_opt "symbol_args" kvs with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result decode_call_arg xs
      in
      Ok (Ir.Ca_symbol_call { Ir.scl_symbol = symbol; scl_args = args })
  | ("ctor", _) :: _ | ("fields", _) :: _ ->
      let* c = decode_call_ctor kvs in
      Ok (Ir.Ca_ctor c)
  | _ ->
      err
        "call arg must be a single param, field, lit, list, map, call, symbol, \
         type, or ctor/fields pair"

and decode_call_ctor kvs =
  let* name =
    match List.assoc_opt "ctor" kvs with
    | Some v -> as_string v
    | None -> err "call ctor is missing name"
  in
  let* fields_j =
    match List.assoc_opt "fields" kvs with
    | Some v -> as_assoc v
    | None -> err "call ctor is missing fields"
  in
  let* fields =
    map_result
      (fun (n, v) ->
        let* a = decode_call_arg v in
        Ok (n, a))
      fields_j
  in
  Ok ({ Ir.cc_name = name; cc_fields = fields } : Ir.call_ctor)

and decode_entry_call j =
  let* kvs = as_assoc j in
  let* ns =
    match List.assoc_opt "ns" kvs with
    | Some v -> as_string v
    | None -> err "call is missing ns"
  in
  let* fn =
    match List.assoc_opt "fn" kvs with
    | Some v -> as_string v
    | None -> err "call is missing fn"
  in
  let* args =
    match List.assoc_opt "args" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_call_arg xs
  in
  Ok ({ Ir.ec_ns = ns; ec_fn = fn; ec_args = args } : Ir.entry_call)

let decode_op_impl_call (j : Ir.json) : (Ir.op_impl_call, string) result =
  let* kvs = as_assoc j in
  match
    ( List.assoc_opt "recv" kvs,
      List.assoc_opt "method" kvs,
      List.assoc_opt "args" kvs )
  with
  | Some recv, Some method_, Some args ->
      let* recv_xs = as_list recv in
      let* oic_recv = map_result as_string recv_xs in
      let* oic_method = as_string method_ in
      let* args_xs = as_list args in
      let* oic_args = map_result decode_call_arg args_xs in
      Ok ({ Ir.oic_recv; oic_method; oic_args } : Ir.op_impl_call)
  | _ -> err "handle call must have recv, method, and args"

let decode_entry_field j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* name =
    match get "name" with
    | Some v -> as_string v
    | None -> err "entry field is missing name"
  in
  let* target =
    match get "target" with
    | Some v -> decode_tref v
    | None -> err "entry field is missing target"
  in
  let* sources =
    match get "sources" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_source xs
  in
  (* A present "format" key means a derivation exists; an absent key means
     there is none (same convention as a member's "default"). *)
  let* format =
    match get "format" with
    | None -> Ok None
    | Some v ->
        let* xs = as_list v in
        let* parts = map_result decode_template_part xs in
        Ok (Some parts)
  in
  let* transforms =
    match get "transforms" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result as_string xs
  in
  let* select =
    match get "select" with
    | None -> Ok None
    | Some v ->
        let* s = decode_select v in
        Ok (Some s)
  in
  let* call =
    match get "call" with
    | None -> Ok None
    | Some v ->
        let* c = decode_entry_call v in
        Ok (Some c)
  in
  let* handle_call =
    match get "handle_call" with
    | None -> Ok None
    | Some v ->
        let* c = decode_op_impl_call v in
        Ok (Some c)
  in
  let* binds =
    match get "binds" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_bind xs
  in
  let* constraints =
    match get "constraints" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_constraint xs
  in
  let* traits =
    match get "traits" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_trait xs
  in
  Ok
    ({
       ef_name = name;
       ef_target = target;
       ef_sources = sources;
       ef_format = format;
       ef_transforms = transforms;
       ef_select = select;
       ef_call = call;
       ef_handle_call = handle_call;
       ef_binds = binds;
       ef_constraints = constraints;
       ef_traits = traits;
     }
      : Ir.entry_field)
