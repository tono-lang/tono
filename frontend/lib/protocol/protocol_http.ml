(* The HTTP Protocol resolver. See the interface for the seam it sits in. *)

type part = Label | Query of string | Header of string | Body | Payload
type response_part = Response_header of string | Response_status_code

type wire_descriptor = {
  http_method : string;
  uri : string;
  bindings : (string * part) list;
  response_bindings : (string * response_part) list;
  success : (int * Ir.tref option) list;
  errors : (int * Ir.shape_id * string option) list;
}

(* ── Reading the lowered trait bag ─────────────────────────────────────── *)

(* The frontend emits bare trait ids today; a future name-resolution pass will
   namespace them as [core#...]. Accept both spellings so hand-authored IR and
   compiled IR resolve identically (mirrors the backend's [find_trait]). *)
let trait_by (id : string) (traits : Ir.trait list) : Ir.trait option =
  let matches (t : Ir.trait) =
    String.equal t.trait_id id || String.equal t.trait_id ("core#" ^ id)
  in
  List.find_opt matches traits

let has_trait id traits = Option.is_some (trait_by id traits)

(* A trait argument the frontend lowered as either a bare scalar or, for a single
   positional arg, a one-element array. *)
let string_arg (v : Ir.json) : string option =
  match v with
  | `String s -> Some s
  | `List (`String s :: _) -> Some s
  | _ -> None

let int_arg (v : Ir.json) : int option =
  match v with
  | `Int n -> Some n
  | `Intlit s -> int_of_string_opt s
  | `List (`Int n :: _) -> Some n
  | _ -> None

let obj_field (k : string) (v : Ir.json) : Ir.json option =
  match v with `Assoc kvs -> List.assoc_opt k kvs | _ -> None

(* ── Binding assignment ────────────────────────────────────────────────── *)

(* The name a query/header binds under: the trait's explicit argument, or the
   member name when the annotation is written bare. *)
let bound_name (member_name : string) (t : Ir.trait) : string =
  Option.value ~default:member_name (string_arg t.value)

let part_of_member (m : Ir.member) : part =
  if has_trait "httpPayload" m.traits then Payload
  else
    match trait_by "httpQuery" m.traits with
    | Some t -> Query (bound_name m.name t)
    | None -> (
        match trait_by "httpHeader" m.traits with
        | Some t -> Header (bound_name m.name t)
        | None -> if has_trait "httpLabel" m.traits then Label else Body)

let response_part_of_member (m : Ir.member) : response_part option =
  if has_trait "httpResponseCode" m.traits then Some Response_status_code
  else
    match trait_by "httpHeader" m.traits with
    | Some t -> Some (Response_header (bound_name m.name t))
    | None -> None

(* The members of the structure a tref points at, or [] when the reference is
   not a resolvable structure (a primitive input, or an unresolved name the
   frontend already reported). *)
let members_of (lookup : Ir.shape_id -> Ir.shape option) (t : Ir.tref option) :
    Ir.member list =
  match t with
  | Some (Ir.Ref (id, _)) -> (
      match lookup id with
      | Some { kind = Ir.Structure { members; _ }; _ } -> members
      | _ -> [])
  | _ -> []

(* ── Error discrimination ──────────────────────────────────────────────── *)

let error_entry (lookup : Ir.shape_id -> Ir.shape option) (t : Ir.tref) :
    (int * Ir.shape_id * string option) option =
  match t with
  | Ir.Ref (id, _) -> (
      match lookup id with
      | Some s ->
          let status =
            Option.bind (trait_by "status" s.traits) (fun t -> int_arg t.value)
          in
          let code =
            Option.bind (trait_by "errorCode" s.traits) (fun t ->
                string_arg t.value)
          in
          Option.map (fun st -> (st, id, code)) status
      | None -> None)
  | _ -> None

(* ── Resolution ────────────────────────────────────────────────────────── *)

let resolve_op (lookup : Ir.shape_id -> Ir.shape option) (op : Ir.shape) :
    wire_descriptor option =
  match (op.kind, trait_by "http" op.traits) with
  | Ir.Operation { input; output; errors }, Some http ->
      let str k default =
        Option.value ~default (Option.bind (obj_field k http.value) string_arg)
      in
      let http_method = String.uppercase_ascii (str "method" "GET") in
      let uri = str "path" "/" in
      let code =
        Option.value ~default:200
          (Option.bind (obj_field "code" http.value) int_arg)
      in
      let bindings =
        List.map
          (fun (m : Ir.member) -> (m.name, part_of_member m))
          (members_of lookup input)
      in
      let response_bindings =
        List.filter_map
          (fun (m : Ir.member) ->
            Option.map (fun p -> (m.name, p)) (response_part_of_member m))
          (members_of lookup output)
      in
      let success = [ (code, output) ] in
      let errors = List.filter_map (error_entry lookup) errors in
      Some { http_method; uri; bindings; response_bindings; success; errors }
  | _ -> None

(* ── JSON encoding (the opaque blob) ───────────────────────────────────── *)

let encode_part : part -> Ir.json = function
  | Label -> `Assoc [ ("kind", `String "label") ]
  | Query name -> `Assoc [ ("kind", `String "query"); ("name", `String name) ]
  | Header name -> `Assoc [ ("kind", `String "header"); ("name", `String name) ]
  | Body -> `Assoc [ ("kind", `String "body") ]
  | Payload -> `Assoc [ ("kind", `String "payload") ]

let encode_response_part : response_part -> Ir.json = function
  | Response_header name ->
      `Assoc [ ("kind", `String "header"); ("name", `String name) ]
  | Response_status_code -> `Assoc [ ("kind", `String "statusCode") ]

let encode (d : wire_descriptor) : Ir.json =
  let pair (name, p) = `List [ `String name; encode_part p ] in
  let rpair (name, p) = `List [ `String name; encode_response_part p ] in
  let succ (status, out) =
    `List
      [
        `Int status;
        (match out with Some t -> Ir_json.encode_tref t | None -> `Null);
      ]
  in
  let err (status, id, code) =
    `List
      [
        `Int status;
        `String id;
        (match code with Some c -> `String c | None -> `Null);
      ]
  in
  `Assoc
    [
      ("http_method", `String d.http_method);
      ("uri", `String d.uri);
      ("bindings", `List (List.map pair d.bindings));
      ("response_bindings", `List (List.map rpair d.response_bindings));
      ("success", `List (List.map succ d.success));
      ("errors", `List (List.map err d.errors));
    ]

(* ── Module pass ───────────────────────────────────────────────────────── *)

let resolve_module (m : Ir.module_) : Ir.module_ =
  let lookup id =
    List.find_opt (fun (s : Ir.shape) -> String.equal s.id id) m.shapes
  in
  let attach (op : Ir.shape) : Ir.shape =
    match resolve_op lookup op with
    | None -> op
    | Some desc ->
        {
          op with
          traits =
            op.traits
            @ [ { Ir.trait_id = "wire_descriptor"; value = encode desc } ];
        }
  in
  { m with operations = List.map attach m.operations }
