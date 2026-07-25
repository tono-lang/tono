(* The HTTP Protocol resolver. See the interface for the seam it sits in. *)

type part = Label | Query of string | Header of string | Body | Payload
type response_part = Response_header of string | Response_status_code

type value_expr =
  | Vlit of Ir.json
  | Vfield of string list
  | Vtemplate of Ir.template_part list

type wire_descriptor = {
  http_method : string;
  uri : string;
  bindings : (string * part) list;
  response_bindings : (string * response_part) list;
  success : (int * Ir.tref option) list;
  errors : (int * Ir.shape_id * string option) list;
  endpoint : string list option;
  request_headers : (Ir.template_part list * value_expr) list;
  timeout : string list option;
  retry : string list option;
}

(* ── Reading the lowered trait bag ─────────────────────────────────────── *)

(* The frontend emits bare trait ids today; a future name-resolution pass will
   namespace them as [core#...]. Accept both spellings so hand-authored IR and
   compiled IR resolve identically (mirrors the backend's [find_trait]). *)
let trait_matches (id : string) (t : Ir.trait) : bool =
  String.equal t.trait_id id || String.equal t.trait_id ("core#" ^ id)

let trait_by (id : string) (traits : Ir.trait list) : Ir.trait option =
  List.find_opt (trait_matches id) traits

let traits_all (id : string) (traits : Ir.trait list) : Ir.trait list =
  List.filter (trait_matches id) traits

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

(* An entry-field reference the frontend lowered as {"field": ["a", "b"]}. *)
let field_path (v : Ir.json) : string list option =
  match v with
  | `Assoc [ ("field", `List segs) ] ->
      let strs =
        List.filter_map (function `String s -> Some s | _ -> None) segs
      in
      if List.length strs = List.length segs then Some strs else None
  | _ -> None

(* Parse a template string into IR parts; the diagnostics sink is discarded
   because the frontend already validated the string at the AST level. *)
let template_of (s : string) : Ir.template_part list =
  let d = ref [] in
  let dpos : Span.pos = { line = 0; col = 0; offset = 0 } in
  let dspan : Span.span = { start = dpos; finish = dpos } in
  Lower.parse_template ~diags:d ~span:dspan s

let has_placeholder (s : string) : bool =
  match template_of s with [] | [ Ir.Tpl_lit _ ] -> false | _ -> true

(* A protocol trait value position: a structured field ref, a template-bearing
   string, or a plain literal. *)
let value_expr_of (j : Ir.json) : value_expr =
  match field_path j with
  | Some p -> Vfield p
  | None -> (
      match j with
      | `String s when has_placeholder s -> Vtemplate (template_of s)
      | other -> Vlit other)

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
      (* Entry-scoped positions: the endpoint ref on @http, op-level @header
         key/value pairs, and the @timeout/@retry field refs. The single
         positional argument of @timeout/@retry arrives as a one-element array
         (the frontend's uniform lowering). *)
      let endpoint = Option.bind (obj_field "endpoint" http.value) field_path in
      let request_headers =
        List.filter_map
          (fun (t : Ir.trait) ->
            match t.value with
            | `List [ `String key; value ] ->
                Some (template_of key, value_expr_of value)
            | _ -> None)
          (traits_all "header" op.traits)
      in
      let single_ref id =
        match trait_by id op.traits with
        | Some { value = `List [ v ]; _ } -> field_path v
        | _ -> None
      in
      let timeout = single_ref "timeout" in
      let retry = single_ref "retry" in
      Some
        {
          http_method;
          uri;
          bindings;
          response_bindings;
          success;
          errors;
          endpoint;
          request_headers;
          timeout;
          retry;
        }
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
  let path segs = `List (List.map (fun s -> `String s) segs) in
  let value_expr = function
    | Vlit j -> `Assoc [ ("lit", j) ]
    | Vfield p -> `Assoc [ ("field", path p) ]
    | Vtemplate parts ->
        `Assoc
          [ ("template", `List (List.map Ir_json.encode_template_part parts)) ]
  in
  let header (key, value) =
    `List
      [ `List (List.map Ir_json.encode_template_part key); value_expr value ]
  in
  let opt_path k = function None -> [] | Some p -> [ (k, path p) ] in
  `Assoc
    ([
       ("http_method", `String d.http_method);
       ("uri", `String d.uri);
       ("bindings", `List (List.map pair d.bindings));
       ("response_bindings", `List (List.map rpair d.response_bindings));
       ("success", `List (List.map succ d.success));
       ("errors", `List (List.map err d.errors));
     ]
    @ opt_path "endpoint" d.endpoint
    @ (match d.request_headers with
      | [] -> []
      | hs -> [ ("request_headers", `List (List.map header hs)) ])
    @ opt_path "timeout" d.timeout
    @ opt_path "retry" d.retry)

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
  (* Ops nested in an entry resolve exactly like loose ops; their descriptor
     additionally carries the entry-scoped refs the traits declared. *)
  let shapes =
    List.map
      (fun (s : Ir.shape) ->
        match s.kind with
        | Ir.Entry { fields; operations } ->
            {
              s with
              kind =
                Ir.Entry { fields; operations = List.map attach operations };
            }
        | _ -> s)
      m.shapes
  in
  { m with shapes; operations = List.map attach m.operations }
