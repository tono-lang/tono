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

let encode_select (s : Ir.select) : Ir.json =
  let arm (a : Ir.select_arm) =
    `Assoc
      ((match a.arm_pattern with Some p -> [ ("pattern", p) ] | None -> [])
      @ [ ("value", encode_arm_value a.arm_value) ])
  in
  `Assoc
    [
      ("subject", encode_path s.subject); ("arms", `List (List.map arm s.arms));
    ]

let encode_bind (b : Ir.bind) : Ir.json =
  `Assoc
    [ ("field", `String b.bind_field); ("source", encode_path b.bind_source) ]

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
  | _ -> err "arm value must be a single field, lit, or sources key"

let decode_select j =
  let* kvs = as_assoc j in
  let* subject =
    match List.assoc_opt "subject" kvs with
    | Some v -> decode_path v
    | None -> err "select is missing subject"
  in
  let decode_arm aj =
    let* akvs = as_assoc aj in
    let* () = ensure_only [ "pattern"; "value" ] akvs in
    (* A present null collapses to the wildcard: patterns are scalar literals,
       and the Rust mirror's Option maps null to absent, so both sides must
       read the two spellings identically. *)
    let pattern =
      match List.assoc_opt "pattern" akvs with
      | Some `Null | None -> None
      | p -> p
    in
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
  Ok ({ subject; arms } : Ir.select)

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
       ef_binds = binds;
       ef_constraints = constraints;
       ef_traits = traits;
     }
      : Ir.entry_field)
