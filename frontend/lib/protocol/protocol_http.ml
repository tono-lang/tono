(* The HTTP Protocol resolver. See the interface for the seam it sits in. *)

type response_part = Response_header of string | Response_status_code

type value_expr =
  | Vlit of Ir.json
  | Vfield of string list
  | Vparam of string list
    (* segments into the op's declared parameter; [] is the whole value *)
  | Vtemplate of Ir.template_part list
  | Vctor of (string * value_expr) list
    (* @body's ctor mapper: field name -> value, resolved down to lit/field/
       param/template positions (the struct name itself carries no wire
       meaning; only the target's own field-to-wire-key encoding does) *)
  | Vcall of call_value
(* an extern call read as a @header/@query/@body value;
       Check_entry_ops.check_request_value already rejected every other
       position ".request" could appear in, so this only has to recognize
       the shape, not re-validate it. *)

and call_value = {
  vc_ns : string;
  vc_fn : string;
  vc_args : call_arg_value list;
}

and call_arg_value =
  | Cv_field of string list
  | Cv_param of string list
  | Cv_lit of Ir.json
  | Cv_ctor of (string * call_arg_value) list
  | Cv_request

type resolution = {
  http_method : string;
  uri : value_expr;
  response_bindings : (string * response_part) list;
  success : (int * Ir.tref option) list;
  endpoint : value_expr option;
  request_headers : (Ir.template_part list * value_expr) list;
  query : (Ir.template_part list * value_expr) list;
  body : value_expr option;
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

(* A single int, or every element of a list value if all of them parse as one
   ([code: 201] and [code: [200, 207]] both resolve here). *)
let single_int (v : Ir.json) : int option =
  match v with `Int n -> Some n | `Intlit s -> int_of_string_opt s | _ -> None

let int_list_arg (v : Ir.json) : int list option =
  match v with
  | `Int _ | `Intlit _ -> Option.map (fun n -> [ n ]) (single_int v)
  | `List xs ->
      let ns = List.filter_map single_int xs in
      if List.length ns = List.length xs && ns <> [] then Some ns else None
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
   because the typechecker already validated the string with its real span. *)
let template_of (s : string) : Ir.template_part list =
  let d = ref [] in
  let dpos : Span.pos = { line = 0; col = 0; offset = 0 } in
  let dspan : Span.span = { start = dpos; finish = dpos } in
  Template.parse ~diags:d ~span:dspan s

let has_placeholder (s : string) : bool =
  match template_of s with [] | [ Ir.Tpl_lit _ ] -> false | _ -> true

(* A protocol trait value position: a structured field ref, a template-bearing
   string, or a plain literal. [pname], when the op declares a named
   parameter, distinguishes an op-parameter reference from an entry-field one:
   the parser/typechecker don't know the difference (both are ".x" syntax),
   so this is the one place that rewrites a [Vfield]/[Tpl_field] whose head
   segment names the parameter into [Vparam]/[Tpl_param] of the remaining
   segments (the whole parameter when there are none left). An op-param name
   shadows a same-named entry field, ordinary lexical shadowing. *)
let rewrite_param_path (pname : string option) (segs : string list) :
    [ `Field of string list | `Param of string list ] =
  match (pname, segs) with
  | Some p, head :: rest when String.equal p head -> `Param rest
  | _ -> `Field segs

let rewrite_param_template (pname : string option)
    (parts : Ir.template_part list) : Ir.template_part list =
  List.map
    (function
      | Ir.Tpl_field segs -> (
          match rewrite_param_path pname segs with
          | `Field segs -> Ir.Tpl_field segs
          | `Param segs -> Ir.Tpl_param segs)
      | other -> other)
    parts

(* A ctor mapper the frontend lowered as {"ctor": name, "fields": {...}}
   (Lower.json_of_arg's [ACtor] case). The struct name itself carries no
   wire meaning at this layer; only the field name -> value mapping does. *)
let ctor_fields (v : Ir.json) : (string * Ir.json) list option =
  match v with
  | `Assoc [ ("ctor", `String _); ("fields", `Assoc kvs) ] -> Some kvs
  | _ -> None

(* An extern call the frontend lowered as
   {"call": {"ns": ..., "fn": ...}, "args": [...]} (Lower.json_of_arg's
   [ACall] case). Each argument is a call_arg's own encoding: {"param": ...}
   (only reachable if Check_entry_ops.check_request_value didn't already
   reject a bare identifier here — kept total rather than asserting),
   {"field": ["request"]} for the reserved canonical-request marker,
   {"field": [...]} for an ordinary ref, or {"ctor": ...}. *)
let call_of (j : Ir.json) : (string * string * Ir.json list) option =
  match j with
  | `Assoc
      [
        ("call", `Assoc [ ("ns", `String ns); ("fn", `String fn) ]);
        ("args", `List args);
      ] ->
      Some (ns, fn, args)
  | _ -> None

let rec call_arg_value_of ?(pname : string option) (j : Ir.json) :
    call_arg_value =
  match j with
  | `Assoc [ ("param", `String n) ] -> Cv_param [ n ]
  | _ -> (
      match field_path j with
      | Some [ "request" ] -> Cv_request
      | Some p -> (
          match rewrite_param_path pname p with
          | `Field segs -> Cv_field segs
          | `Param segs -> Cv_param segs)
      | None -> (
          match ctor_fields j with
          | Some kvs ->
              Cv_ctor
                (List.map (fun (n, v) -> (n, call_arg_value_of ?pname v)) kvs)
          | None -> Cv_lit j))

let call_value_of ?pname (ns, fn, args) : call_value =
  { vc_ns = ns; vc_fn = fn; vc_args = List.map (call_arg_value_of ?pname) args }

let rec value_expr_of ?(pname : string option) (j : Ir.json) : value_expr =
  match call_of j with
  | Some c -> Vcall (call_value_of ?pname c)
  | None -> (
      match field_path j with
      | Some p -> (
          match rewrite_param_path pname p with
          | `Field segs -> Vfield segs
          | `Param segs -> Vparam segs)
      | None -> (
          match ctor_fields j with
          | Some kvs ->
              Vctor (List.map (fun (n, v) -> (n, value_expr_of ?pname v)) kvs)
          | None -> (
              match j with
              | `String s when has_placeholder s ->
                  Vtemplate (rewrite_param_template pname (template_of s))
              | other -> Vlit other)))

(* ── Binding assignment ────────────────────────────────────────────────── *)

(* The name a query/header binds under: the trait's explicit argument, or the
   member name when the annotation is written bare. *)
let bound_name (member_name : string) (t : Ir.trait) : string =
  Option.value ~default:member_name (string_arg t.value)

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

(* ── Resolution ────────────────────────────────────────────────────────── *)

let resolve_op (lookup : Ir.shape_id -> Ir.shape option) (op : Ir.shape) :
    resolution option =
  match (op.kind, trait_by "http" op.traits) with
  | Ir.Operation { input; input_name = pname; output; _ }, Some http ->
      let str k default =
        Option.value ~default (Option.bind (obj_field k http.value) string_arg)
      in
      let value ?default k =
        match obj_field k http.value with
        | Some j -> value_expr_of ?pname j
        | None -> ( match default with Some v -> v | None -> Vlit `Null)
      in
      let http_method = String.uppercase_ascii (str "method" "GET") in
      let uri = value "path" ~default:(Vlit (`String "/")) in
      let codes = Option.bind (obj_field "code" http.value) int_list_arg in
      ignore input;
      let response_bindings =
        List.filter_map
          (fun (m : Ir.member) ->
            Option.map (fun p -> (m.name, p)) (response_part_of_member m))
          (members_of lookup output)
      in
      (* No [code:] leaves [success] empty: the resolved [wb_success] stays
         [] and every emitter falls back to the 2xx-range convention. A
         declared [code:] (single or a list) makes [wb_success] the exact
         set of statuses that count as success. *)
      let success =
        match codes with
        | Some cs -> List.map (fun c -> (c, output)) cs
        | None -> []
      in
      (* Entry-scoped positions: the endpoint ref on @http, op-level @header
         key/value pairs, and the @timeout/@retry field refs. The single
         positional argument of @timeout/@retry arrives as a one-element array
         (the frontend's uniform lowering). *)
      let endpoint =
        Option.map (value_expr_of ?pname) (obj_field "endpoint" http.value)
      in
      let kv_traits name =
        List.filter_map
          (fun (t : Ir.trait) ->
            match t.value with
            | `List [ `String key; v ] ->
                Some
                  ( rewrite_param_template pname (template_of key),
                    value_expr_of ?pname v )
            | _ -> None)
          (traits_all name op.traits)
      in
      let request_headers = kv_traits "header" in
      let query = kv_traits "query" in
      let body =
        match trait_by "body" op.traits with
        | Some { value = `List [ v ]; _ } -> Some (value_expr_of ?pname v)
        | _ -> None
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
          response_bindings;
          success;
          endpoint;
          request_headers;
          query;
          body;
          timeout;
          retry;
        }
  | _ -> None

(* ── Resolved wire binding (the typed IR field) ────────────────────────── *)

let to_wire_response_part : response_part -> Ir.wire_response_part = function
  | Response_header n -> Ir.Wire_response_header n
  | Response_status_code -> Ir.Wire_response_status_code

let rec to_wire_call_arg : call_arg_value -> Ir.wire_call_arg = function
  | Cv_field p -> Ir.Wca_field p
  | Cv_param p -> Ir.Wca_param p
  | Cv_lit j -> Ir.Wca_lit j
  | Cv_ctor fields ->
      Ir.Wca_ctor (List.map (fun (n, v) -> (n, to_wire_call_arg v)) fields)
  | Cv_request -> Ir.Wca_request

let to_wire_call (c : call_value) : Ir.wire_call =
  {
    Ir.wcl_ns = c.vc_ns;
    wcl_fn = c.vc_fn;
    wcl_args = List.map to_wire_call_arg c.vc_args;
  }

let rec to_wire_value : value_expr -> Ir.wire_value = function
  | Vlit j -> Ir.Wire_lit j
  | Vfield p -> Ir.Wire_field p
  | Vparam p -> Ir.Wire_param p
  | Vtemplate t -> Ir.Wire_template t
  | Vctor fields ->
      Ir.Wire_object (List.map (fun (n, v) -> (n, to_wire_value v)) fields)
  | Vcall c -> Ir.Wire_call (to_wire_call c)

(* The typed IR value for [Ir.wire_binding]: the resolution minus the dead
   weight (the errors array duplicates the operation's own [errors] field and
   the referenced shapes' own status/errorCode/retryable traits, which the
   backend's error taxonomy already reads directly; the success tref is
   discarded by every emitter today) and with [timeout]/[retry] kept as the
   plain entry-field path, so a target can resolve them at the call site
   instead of a string-keyed lookup. *)
let to_ir_binding (d : resolution) : Ir.wire_binding =
  {
    Ir.wb_method = d.http_method;
    wb_uri = to_wire_value d.uri;
    wb_body = Option.map to_wire_value d.body;
    wb_response_bindings =
      List.map (fun (n, p) -> (n, to_wire_response_part p)) d.response_bindings;
    wb_success = List.map fst d.success;
    wb_endpoint = Option.map to_wire_value d.endpoint;
    wb_request_headers =
      List.map (fun (k, v) -> (k, to_wire_value v)) d.request_headers;
    wb_query = List.map (fun (k, v) -> (k, to_wire_value v)) d.query;
    wb_timeout = d.timeout;
    wb_retry = d.retry;
  }

(* ── Module pass ───────────────────────────────────────────────────────── *)

let resolve_module (m : Ir.module_) : Ir.module_ =
  let lookup id =
    List.find_opt (fun (s : Ir.shape) -> String.equal s.id id) m.shapes
  in
  let attach (op : Ir.shape) : Ir.shape =
    match resolve_op lookup op with
    | None -> op
    | Some desc ->
        let kind =
          match op.kind with
          | Ir.Operation o ->
              Ir.Operation { o with wire = Some (to_ir_binding desc) }
          | other -> other
          (* unreachable: resolve_op only returns Some for an Operation *)
        in
        { op with kind }
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
