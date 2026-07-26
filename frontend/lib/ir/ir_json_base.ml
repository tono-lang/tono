(* Scalar-level JSON codecs for the IR: primitives, type references, core
   constraints, traits, members, and enum values, plus the untrusted-input
   coercion helpers every decoder shares. [Ir_json] composes these into the
   shape/module/model layer; the split keeps each file within the size
   ceiling without touching the wire format. *)

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
