(* JSON wire encoding for the IR. This is the contract the Rust backend mirrors.
   Rules:
   - primitives are bare strings ("i32", "string", ...);
   - a [tref] is a single-key tagged object, except [ref] which carries a sibling
     "args" array ({"ref": <id>, "args": [...]});
   - a core constraint is a single-key tagged object with camelCase fields;
   - a [shape] is internally tagged by a "kind" field flattened next to "id" and
     "traits";
   - the envelope carries a bare integer "tono_ir_version" gate.
   Encoders assume well-formed in-memory values and raise [Ir.Invalid_ir] on the
   few things JSON cannot represent. Decoders take untrusted input and return a
   [result]. *)

(* The IR schema revision this build understands. Bumped by one on every
   incompatible change to the wire format; there is no negotiation.
   v2 removed the enum [open] field (every enum is open).
   v3 added the module [extensions] table (bespoke hooks/contracts/constraints).
   v4 made an enum value an object ({"name", "value"?, "traits"}) so it can carry
   a trait bag (documentation rides it), replacing the [name, intOrNull] pair.
   v5 added the entry model: "entry"/"config" shape kinds whose fields carry
   value sources, @format templates, transform pipelines, match selection, and
   @bind composition; entry ops nest inside the entry shape and their trait
   values may carry field references ({"field": [...]}). *)
let current_ir_version = 5

(* ── Encoding ──────────────────────────────────────────────────────────── *)

let encode_prim (p : Ir.prim) : Ir.json =
  let s =
    match p with
    | Bool -> "bool"
    | String -> "string"
    | Bytes -> "bytes"
    | Float -> "float"
    | Timestamp -> "timestamp"
    | Date -> "date"
    | Duration -> "duration"
    | Uuid -> "uuid"
    | Int { bits; signed } ->
        if not (List.mem bits Ir.valid_int_bits) then
          raise
            (Ir.Invalid_ir
               (Printf.sprintf
                  "integer bit width %d is not one of 8, 16, 32, 64" bits));
        (if signed then "i" else "u") ^ string_of_int bits
  in
  `String s

let encode_trait (t : Ir.trait) : Ir.json =
  `Assoc [ ("id", `String t.trait_id); ("value", t.value) ]

let encode_constraint (c : Ir.constraint_) : Ir.json =
  match c with
  | Range { min; max; excl_min; excl_max } ->
      let num k = function
        | None -> []
        | Some f -> [ (k, `Float (Ir.finite "Range bound" f)) ]
      in
      `Assoc
        [
          ( "range",
            `Assoc
              (num "min" min @ num "max" max
              @ [ ("exclMin", `Bool excl_min); ("exclMax", `Bool excl_max) ]) );
        ]
  | Length { min; max } ->
      let num k = function None -> [] | Some i -> [ (k, `Int i) ] in
      `Assoc [ ("length", `Assoc (num "min" min @ num "max" max)) ]
  | Pattern s -> `Assoc [ ("pattern", `String s) ]
  | MultipleOf f -> `Assoc [ ("multipleOf", `Float (Ir.finite "MultipleOf" f)) ]
  | Custom _ ->
      raise
        (Ir.Invalid_ir
           "custom constraint must live in the trait bag, not in constraints")

let encode_enum_value (v : Ir.enum_value) : Ir.json =
  `Assoc
    (("name", `String v.ev_name)
     :: (match v.ev_int with Some i -> [ ("value", `Int i) ] | None -> [])
    @ [ ("traits", `List (List.map encode_trait v.ev_traits)) ])

let encode_backing = function `String -> "string" | `Int -> "int"

(* ── Entry-model encoding ──────────────────────────────────────────────── *)

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

let rec encode_tref (t : Ir.tref) : Ir.json =
  match t with
  | Prim p -> `Assoc [ ("prim", encode_prim p) ]
  | Ref (id, args) ->
      `Assoc
        [ ("ref", `String id); ("args", `List (List.map encode_tref args)) ]
  | Param s -> `Assoc [ ("param", `String s) ]
  | List t -> `Assoc [ ("list", encode_tref t) ]
  | Map (k, v) -> `Assoc [ ("map", `List [ encode_tref k; encode_tref v ]) ]

and encode_member (m : Ir.member) : Ir.json =
  `Assoc
    ([
       ("name", `String m.name);
       ("target", encode_tref m.target);
       ("required", `Bool m.required);
     ]
    @ (match m.default with None -> [] | Some v -> [ ("default", v) ])
    @ [
        ("constraints", `List (List.map encode_constraint m.constraints));
        ("traits", `List (List.map encode_trait m.traits));
      ])

and encode_shape_kind_fields (k : Ir.shape_kind) : (string * Ir.json) list =
  let params ps = `List (List.map (fun p -> `String p) ps) in
  let members ms = `List (List.map encode_member ms) in
  match k with
  | Structure { params = ps; members = ms } ->
      [
        ("kind", `String "structure");
        ("params", params ps);
        ("members", members ms);
      ]
  | Union { params = ps; members = ms; discriminator } ->
      [
        ("kind", `String "union");
        ("params", params ps);
        ("members", members ms);
        ("discriminator", `String discriminator);
      ]
  | Enum { backing; values } ->
      [
        ("kind", `String "enum");
        ("backing", `String (encode_backing backing));
        ("values", `List (List.map encode_enum_value values));
      ]
  | Service { operations } ->
      [
        ("kind", `String "service");
        ("operations", `List (List.map (fun s -> `String s) operations));
      ]
  | Operation { input; output; errors } ->
      let opt = function None -> `Null | Some t -> encode_tref t in
      [
        ("kind", `String "operation");
        ("input", opt input);
        ("output", opt output);
        ("errors", `List (List.map encode_tref errors));
      ]
  | Entry { fields; operations } ->
      [
        ("kind", `String "entry");
        ("fields", `List (List.map encode_entry_field fields));
        ("operations", `List (List.map encode_shape operations));
      ]
  | Config { fields } ->
      [
        ("kind", `String "config");
        ("fields", `List (List.map encode_entry_field fields));
      ]

and encode_entry_field (f : Ir.entry_field) : Ir.json =
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

and encode_shape (s : Ir.shape) : Ir.json =
  `Assoc
    ((("id", `String s.id) :: encode_shape_kind_fields s.kind)
    @ [ ("traits", `List (List.map encode_trait s.traits)) ])

let encode_ext_kind = function
  | Ir.Hook -> "hook"
  | Ir.Contract -> "contract"
  | Ir.Constraint -> "constraint"

let encode_ext_sig (s : Ir.ext_sig) : Ir.json =
  `Assoc [ ("input", encode_tref s.input); ("output", encode_tref s.output) ]

let encode_binding (b : Ir.binding) : Ir.json =
  `Assoc (List.map (fun (lang, target) -> (lang, `String target)) b)

let encode_extension (e : Ir.extension) : Ir.json =
  `Assoc
    ([
       ("name", `String e.ext_name);
       ("kind", `String (encode_ext_kind e.ext_kind));
     ]
    @ (match e.ext_sig with
      | None -> []
      | Some s -> [ ("signature", encode_ext_sig s) ])
    @ [ ("bindings", encode_binding e.ext_bindings) ]
    @
    match e.ext_conformance with
    | None -> []
    | Some c -> [ ("conformance", `String c) ])

let encode_module (m : Ir.module_) : Ir.json =
  `Assoc
    [
      ("name", `String m.mod_name);
      ("shapes", `List (List.map encode_shape m.shapes));
      ("operations", `List (List.map encode_shape m.operations));
      ("extensions", `List (List.map encode_extension m.extensions));
    ]

let encode_model (m : Ir.model) : Ir.json =
  `Assoc
    [
      ("tono_ir_version", `Int m.tono_ir_version);
      ("modules", `List (List.map encode_module m.modules));
    ]

(* ── Decoding ──────────────────────────────────────────────────────────── *)

let ( let* ) = Result.bind
let err fmt = Printf.ksprintf (fun s -> Error s) fmt

let rec map_result f = function
  | [] -> Ok []
  | x :: xs ->
      let* y = f x in
      let* ys = map_result f xs in
      Ok (y :: ys)

let as_assoc = function `Assoc kvs -> Ok kvs | _ -> err "expected an object"
let as_list = function `List xs -> Ok xs | _ -> err "expected an array"
let as_string = function `String s -> Ok s | _ -> err "expected a string"
let as_bool = function `Bool b -> Ok b | _ -> err "expected a boolean"

let as_int = function
  | `Int i -> Ok i
  | `Intlit s -> (
      match int_of_string_opt s with
      | Some i -> Ok i
      | None -> err "integer out of range: %s" s)
  | _ -> err "expected an integer"

(* JSON has no NaN or infinity, and the encoder rejects non-finite floats, so a
   value that parses to one (e.g. an overflowing literal like 1e999) is refused
   here rather than being accepted on decode and then crashing on re-encode. *)
let as_finite_float f =
  if Float.is_finite f then Ok f else err "number is not finite"

let as_float = function
  | `Int i -> Ok (float_of_int i)
  | `Float f -> as_finite_float f
  | `Intlit s -> (
      match float_of_string_opt s with
      | Some f -> as_finite_float f
      | None -> err "not a number: %s" s)
  | _ -> err "expected a number"

let ensure_only allowed kvs =
  match List.find_opt (fun (k, _) -> not (List.mem k allowed)) kvs with
  | None -> Ok ()
  | Some (k, _) -> err "unexpected key %S" k

let int_prim_of_string = function
  | "i8" -> Some (8, true)
  | "i16" -> Some (16, true)
  | "i32" -> Some (32, true)
  | "i64" -> Some (64, true)
  | "u8" -> Some (8, false)
  | "u16" -> Some (16, false)
  | "u32" -> Some (32, false)
  | "u64" -> Some (64, false)
  | _ -> None

let decode_prim j =
  let* s = as_string j in
  match s with
  | "bool" -> Ok Ir.Bool
  | "string" -> Ok Ir.String
  | "bytes" -> Ok Ir.Bytes
  | "float" -> Ok Ir.Float
  | "timestamp" -> Ok Ir.Timestamp
  | "date" -> Ok Ir.Date
  | "duration" -> Ok Ir.Duration
  | "uuid" -> Ok Ir.Uuid
  | other -> (
      match int_prim_of_string other with
      | Some (bits, signed) -> Ok (Ir.Int { bits; signed })
      | None -> err "unknown primitive %S" other)

let tref_keys = [ "prim"; "ref"; "param"; "list"; "map" ]

let rec decode_tref j =
  let* kvs = as_assoc j in
  match List.filter (fun (k, _) -> List.mem k tref_keys) kvs with
  | [ ("prim", v) ] ->
      let* () = ensure_only [ "prim" ] kvs in
      let* p = decode_prim v in
      Ok (Ir.Prim p)
  | [ ("param", v) ] ->
      let* () = ensure_only [ "param" ] kvs in
      let* s = as_string v in
      Ok (Ir.Param s)
  | [ ("list", v) ] ->
      let* () = ensure_only [ "list" ] kvs in
      let* t = decode_tref v in
      Ok (Ir.List t)
  | [ ("map", v) ] -> (
      let* () = ensure_only [ "map" ] kvs in
      let* xs = as_list v in
      match xs with
      | [ a; b ] ->
          let* ka = decode_tref a in
          let* vb = decode_tref b in
          Ok (Ir.Map (ka, vb))
      | _ -> err "map expects a 2-element array")
  | [ ("ref", v) ] ->
      let* () = ensure_only [ "ref"; "args" ] kvs in
      let* id = as_string v in
      let* args =
        match List.assoc_opt "args" kvs with
        | None -> err "ref is missing args"
        | Some a ->
            let* xs = as_list a in
            map_result decode_tref xs
      in
      Ok (Ir.Ref (id, args))
  | [] -> err "tref object has no recognized variant key"
  | _ -> err "tref object has multiple variant keys"

let constraint_keys = [ "range"; "length"; "pattern"; "multipleOf" ]

let decode_constraint j =
  let* kvs = as_assoc j in
  match List.filter (fun (k, _) -> List.mem k constraint_keys) kvs with
  | [ ("range", v) ] ->
      let* () = ensure_only [ "range" ] kvs in
      let* o = as_assoc v in
      let get k = List.assoc_opt k o in
      let float_opt k =
        match get k with
        | None -> Ok None
        | Some x ->
            let* f = as_float x in
            Ok (Some f)
      in
      let bool_flag k =
        match get k with
        | None -> Ok false
        | Some (`Bool b) -> Ok b
        | Some _ -> err "%s must be a boolean" k
      in
      let* min = float_opt "min" in
      let* max = float_opt "max" in
      let* excl_min = bool_flag "exclMin" in
      let* excl_max = bool_flag "exclMax" in
      Ok (Ir.Range { min; max; excl_min; excl_max })
  | [ ("length", v) ] ->
      let* () = ensure_only [ "length" ] kvs in
      let* o = as_assoc v in
      let get k = List.assoc_opt k o in
      let opt k =
        match get k with
        | None -> Ok None
        | Some x ->
            let* i = as_int x in
            Ok (Some i)
      in
      let* min = opt "min" in
      let* max = opt "max" in
      Ok (Ir.Length { min; max })
  | [ ("pattern", v) ] ->
      let* () = ensure_only [ "pattern" ] kvs in
      let* s = as_string v in
      Ok (Ir.Pattern s)
  | [ ("multipleOf", v) ] ->
      let* () = ensure_only [ "multipleOf" ] kvs in
      let* f = as_float v in
      Ok (Ir.MultipleOf f)
  | [] -> err "constraint object has no recognized key"
  | _ -> err "constraint object has multiple keys"

let decode_trait j =
  let* kvs = as_assoc j in
  let* id =
    match List.assoc_opt "id" kvs with
    | Some v -> as_string v
    | None -> err "trait is missing id"
  in
  let* value =
    match List.assoc_opt "value" kvs with
    | Some v -> Ok v
    | None -> err "trait is missing value"
  in
  Ok ({ trait_id = id; value } : Ir.trait)

let decode_member j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* name =
    match get "name" with
    | Some v -> as_string v
    | None -> err "member is missing name"
  in
  let* target =
    match get "target" with
    | Some v -> decode_tref v
    | None -> err "member is missing target"
  in
  let* required =
    match get "required" with
    | Some v -> as_bool v
    | None -> err "member is missing required"
  in
  (* A present "default" key (even null) means a default exists; an absent key
     means there is none. *)
  let default = get "default" in
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
  Ok ({ name; target; required; default; constraints; traits } : Ir.member)

let decode_enum_value j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* name =
    match get "name" with
    | Some v -> as_string v
    | None -> err "enum value is missing name"
  in
  (* An absent (or null) "value" is a string-backed member with no discriminant;
     an int-backed one carries its integer here. *)
  let* value =
    match get "value" with
    | None | Some `Null -> Ok None
    | Some v ->
        let* i = as_int v in
        Ok (Some i)
  in
  let* traits =
    match get "traits" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_trait xs
  in
  Ok ({ ev_name = name; ev_int = value; ev_traits = traits } : Ir.enum_value)

let decode_tref_opt = function
  | None | Some `Null -> Ok None
  | Some v ->
      let* t = decode_tref v in
      Ok (Some t)

(* ── Entry-model decoding ──────────────────────────────────────────────── *)

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
  | [ ("input", v) ] ->
      let* s = as_string v in
      Ok (Ir.Tpl_input s)
  | _ -> err "template part must be a single lit, field, or input key"

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

let rec decode_shape_kind kvs =
  let get k = List.assoc_opt k kvs in
  let* kind =
    match get "kind" with
    | Some v -> as_string v
    | None -> err "shape is missing kind"
  in
  let params () =
    match get "params" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result as_string xs
  in
  let members () =
    match get "members" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_member xs
  in
  match kind with
  | "structure" ->
      let* params = params () in
      let* members = members () in
      Ok (Ir.Structure { params; members })
  | "union" ->
      let* params = params () in
      let* members = members () in
      let* discriminator =
        match get "discriminator" with
        | None -> Ok "type"
        | Some (`String s) -> Ok s
        | Some _ -> err "union discriminator must be a string"
      in
      Ok (Ir.Union { params; members; discriminator })
  | "enum" ->
      let* backing =
        match get "backing" with
        | Some (`String "string") -> Ok `String
        | Some (`String "int") -> Ok `Int
        | Some _ -> err "enum backing must be \"string\" or \"int\""
        | None -> err "enum is missing backing"
      in
      let* values =
        match get "values" with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result decode_enum_value xs
      in
      Ok (Ir.Enum { backing; values })
  | "service" ->
      let* operations =
        match get "operations" with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result as_string xs
      in
      Ok (Ir.Service { operations })
  | "operation" ->
      let* input = decode_tref_opt (get "input") in
      let* output = decode_tref_opt (get "output") in
      let* errors =
        match get "errors" with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result decode_tref xs
      in
      Ok (Ir.Operation { input; output; errors })
  | "entry" ->
      let* fields = decode_fields (get "fields") in
      let* operations =
        match get "operations" with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result decode_shape xs
      in
      Ok (Ir.Entry { fields; operations })
  | "config" ->
      let* fields = decode_fields (get "fields") in
      Ok (Ir.Config { fields })
  | other -> err "unknown shape kind %S" other

and decode_fields = function
  | None -> Ok []
  | Some v ->
      let* xs = as_list v in
      map_result decode_entry_field xs

and decode_entry_field j =
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

and decode_shape j =
  let* kvs = as_assoc j in
  let* id =
    match List.assoc_opt "id" kvs with
    | Some v -> as_string v
    | None -> err "shape is missing id"
  in
  let* kind = decode_shape_kind kvs in
  let* traits =
    match List.assoc_opt "traits" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_trait xs
  in
  Ok ({ id; kind; traits } : Ir.shape)

let decode_ext_kind j =
  let* s = as_string j in
  match s with
  | "hook" -> Ok Ir.Hook
  | "contract" -> Ok Ir.Contract
  | "constraint" -> Ok Ir.Constraint
  | other -> err "unknown extension kind %S" other

let decode_ext_sig j =
  let* kvs = as_assoc j in
  let field k =
    match List.assoc_opt k kvs with
    | Some v -> decode_tref v
    | None -> err "extension signature is missing %s" k
  in
  let* input = field "input" in
  let* output = field "output" in
  Ok ({ input; output } : Ir.ext_sig)

let decode_binding j =
  let* kvs = as_assoc j in
  map_result
    (fun (lang, v) ->
      let* target = as_string v in
      Ok (lang, target))
    kvs

let decode_extension j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* ext_name =
    match get "name" with
    | Some v -> as_string v
    | None -> err "extension is missing name"
  in
  let* ext_kind =
    match get "kind" with
    | Some v -> decode_ext_kind v
    | None -> err "extension is missing kind"
  in
  let* ext_sig =
    match get "signature" with
    | None -> Ok None
    | Some v ->
        let* s = decode_ext_sig v in
        Ok (Some s)
  in
  let* ext_bindings =
    match get "bindings" with None -> Ok [] | Some v -> decode_binding v
  in
  let* ext_conformance =
    match get "conformance" with
    | None -> Ok None
    | Some v ->
        let* s = as_string v in
        Ok (Some s)
  in
  Ok
    ({ ext_name; ext_kind; ext_sig; ext_bindings; ext_conformance }
      : Ir.extension)

let decode_module j =
  let* kvs = as_assoc j in
  let* mod_name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "module is missing name"
  in
  let shapes_of k =
    match List.assoc_opt k kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_shape xs
  in
  let* shapes = shapes_of "shapes" in
  let* operations = shapes_of "operations" in
  let* extensions =
    match List.assoc_opt "extensions" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_extension xs
  in
  Ok ({ mod_name; shapes; operations; extensions } : Ir.module_)

let decode_model j =
  let* kvs = as_assoc j in
  let* version =
    match List.assoc_opt "tono_ir_version" kvs with
    | Some v -> as_int v
    | None -> err "model is missing tono_ir_version"
  in
  if version <> current_ir_version then
    err "unsupported tono_ir_version %d (this build supports %d)" version
      current_ir_version
  else
    let* modules =
      match List.assoc_opt "modules" kvs with
      | None -> Ok []
      | Some v ->
          let* xs = as_list v in
          map_result decode_module xs
    in
    Ok ({ tono_ir_version = version; modules } : Ir.model)

(* ── Canonical form (for stable comparison across emitters) ────────────── *)

(* Recursively sorts object keys and collapses [`Intlit] that fits a native int.
   Used to compare JSON produced by different emitters (yojson, serde_json)
   without depending on key order or whitespace. *)
let rec canonicalize (j : Ir.json) : Ir.json =
  match j with
  | `Assoc kvs ->
      `Assoc
        (List.sort
           (fun (a, _) (b, _) -> String.compare a b)
           (List.map (fun (k, v) -> (k, canonicalize v)) kvs))
  | `List xs -> `List (List.map canonicalize xs)
  | `Intlit s -> (
      match int_of_string_opt s with Some i -> `Int i | None -> `Intlit s)
  | (`Null | `Bool _ | `Int _ | `Float _ | `String _) as leaf -> leaf

let to_canonical_string (j : Ir.json) : string =
  Yojson.Safe.to_string (canonicalize j)

(* Re-exported low-level helpers so their edge cases can be tested directly while
   staying out of the intended public surface (see the .mli). *)
module Internal = struct
  let as_int = as_int
  let as_float = as_float
end
