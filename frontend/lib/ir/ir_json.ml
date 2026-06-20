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
   incompatible change to the wire format; there is no negotiation. *)
let current_ir_version = 1

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

let encode_enum_value (name, v) : Ir.json =
  `List [ `String name; (match v with Some i -> `Int i | None -> `Null) ]

let encode_backing = function `String -> "string" | `Int -> "int"

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
  | Enum { backing; values; open_ } ->
      [
        ("kind", `String "enum");
        ("backing", `String (encode_backing backing));
        ("values", `List (List.map encode_enum_value values));
        ("open", `Bool open_);
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

and encode_shape (s : Ir.shape) : Ir.json =
  `Assoc
    ((("id", `String s.id) :: encode_shape_kind_fields s.kind)
    @ [ ("traits", `List (List.map encode_trait s.traits)) ])

let encode_module (m : Ir.module_) : Ir.json =
  `Assoc
    [
      ("name", `String m.mod_name);
      ("shapes", `List (List.map encode_shape m.shapes));
      ("operations", `List (List.map encode_shape m.operations));
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
  let* xs = as_list j in
  match xs with
  | [ n; v ] ->
      let* name = as_string n in
      let* value =
        match v with
        | `Null -> Ok None
        | _ ->
            let* i = as_int v in
            Ok (Some i)
      in
      Ok (name, value)
  | _ -> err "enum value expects a [name, intOrNull] pair"

let decode_tref_opt = function
  | None | Some `Null -> Ok None
  | Some v ->
      let* t = decode_tref v in
      Ok (Some t)

let decode_shape_kind kvs =
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
      let* open_ =
        match get "open" with
        | None -> Ok true
        | Some (`Bool b) -> Ok b
        | Some _ -> err "enum open flag must be a boolean"
      in
      Ok (Ir.Enum { backing; values; open_ })
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
  | other -> err "unknown shape kind %S" other

let decode_shape j =
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
  Ok ({ mod_name; shapes; operations } : Ir.module_)

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
