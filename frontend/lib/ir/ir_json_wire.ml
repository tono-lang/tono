(* JSON codec for the resolved wire binding, the "wire" field on an operation
   shape. [Ir_json] folds this into the operation shape kind. *)

let encode_template_part = Ir_json_entry.encode_template_part
let decode_template_part = Ir_json_entry.decode_template_part
let ( let* ) = Result.bind
let err fmt = Printf.ksprintf (fun s -> Error s) fmt
let map_result = Ir_json_base.map_result
let as_assoc = Ir_json_base.as_assoc
let as_list = Ir_json_base.as_list
let as_string = Ir_json_base.as_string
let as_int = Ir_json_base.as_int

(* ── Encoding ──────────────────────────────────────────────────────────── *)

let encode_path (segs : string list) : Ir.json =
  `List (List.map (fun s -> `String s) segs)

let encode_wire_part : Ir.wire_part -> Ir.json = function
  | Ir.Wire_label -> `Assoc [ ("kind", `String "label") ]
  | Ir.Wire_query name ->
      `Assoc [ ("kind", `String "query"); ("name", `String name) ]
  | Ir.Wire_header name ->
      `Assoc [ ("kind", `String "header"); ("name", `String name) ]
  | Ir.Wire_body -> `Assoc [ ("kind", `String "body") ]
  | Ir.Wire_payload -> `Assoc [ ("kind", `String "payload") ]

let encode_wire_response_part : Ir.wire_response_part -> Ir.json = function
  | Ir.Wire_response_header name ->
      `Assoc [ ("kind", `String "header"); ("name", `String name) ]
  | Ir.Wire_response_status_code -> `Assoc [ ("kind", `String "statusCode") ]

let encode_wire_value : Ir.wire_value -> Ir.json = function
  | Ir.Wire_lit j -> `Assoc [ ("lit", j) ]
  | Ir.Wire_field p -> `Assoc [ ("field", encode_path p) ]
  | Ir.Wire_param p -> `Assoc [ ("param", encode_path p) ]
  | Ir.Wire_template parts ->
      `Assoc [ ("template", `List (List.map encode_template_part parts)) ]

let encode_named_assoc encode_v (xs : (string * 'a) list) : Ir.json =
  `Assoc (List.map (fun (n, v) -> (n, encode_v v)) xs)

let encode_wire_binding (b : Ir.wire_binding) : Ir.json =
  let request_header (key, value) =
    `List [ `List (List.map encode_template_part key); encode_wire_value value ]
  in
  `Assoc
    ([
       ("method", `String b.wb_method);
       ("uri", encode_wire_value b.wb_uri);
       ("bindings", encode_named_assoc encode_wire_part b.wb_bindings);
       ( "response_bindings",
         encode_named_assoc encode_wire_response_part b.wb_response_bindings );
       ("success", `List (List.map (fun s -> `Int s) b.wb_success));
       ("request_headers", `List (List.map request_header b.wb_request_headers));
       ("query", `List (List.map request_header b.wb_query));
     ]
    @ (match b.wb_endpoint with
      | None -> []
      | Some v -> [ ("endpoint", encode_wire_value v) ])
    @ (match b.wb_timeout with
      | None -> []
      | Some p -> [ ("timeout", encode_path p) ])
    @
    match b.wb_retry with
    | None -> []
    | Some p -> [ ("retry", encode_path p) ])

(* ── Decoding ──────────────────────────────────────────────────────────── *)

let decode_path j =
  let* xs = as_list j in
  map_result as_string xs

let decode_wire_part j =
  let* kvs = as_assoc j in
  let* kind =
    match List.assoc_opt "kind" kvs with
    | Some v -> as_string v
    | None -> err "wire part is missing kind"
  in
  let name () =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "wire part %S is missing name" kind
  in
  match kind with
  | "label" -> Ok Ir.Wire_label
  | "query" ->
      let* n = name () in
      Ok (Ir.Wire_query n)
  | "header" ->
      let* n = name () in
      Ok (Ir.Wire_header n)
  | "body" -> Ok Ir.Wire_body
  | "payload" -> Ok Ir.Wire_payload
  | other -> err "unknown wire part kind %S" other

let decode_wire_response_part j =
  let* kvs = as_assoc j in
  let* kind =
    match List.assoc_opt "kind" kvs with
    | Some v -> as_string v
    | None -> err "wire response part is missing kind"
  in
  match kind with
  | "header" -> (
      match List.assoc_opt "name" kvs with
      | Some v ->
          let* n = as_string v in
          Ok (Ir.Wire_response_header n)
      | None -> err "wire response header part is missing name")
  | "statusCode" -> Ok Ir.Wire_response_status_code
  | other -> err "unknown wire response part kind %S" other

let decode_wire_value j =
  let* kvs = as_assoc j in
  match kvs with
  | [ ("lit", v) ] -> Ok (Ir.Wire_lit v)
  | [ ("field", v) ] ->
      let* p = decode_path v in
      Ok (Ir.Wire_field p)
  | [ ("param", v) ] ->
      let* p = decode_path v in
      Ok (Ir.Wire_param p)
  | [ ("template", v) ] ->
      let* xs = as_list v in
      let* parts = map_result decode_template_part xs in
      Ok (Ir.Wire_template parts)
  | _ -> err "wire value must be a single lit, field, param, or template key"

let decode_named_assoc decode_v j =
  let* kvs = as_assoc j in
  map_result
    (fun (n, v) ->
      let* x = decode_v v in
      Ok (n, x))
    kvs

let decode_request_header j =
  match j with
  | `List [ key; value ] ->
      let* kxs = as_list key in
      let* k = map_result decode_template_part kxs in
      let* v = decode_wire_value value in
      Ok (k, v)
  | _ -> err "request header must be a [key, value] pair"

let decode_wire_binding j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* wb_method =
    match get "method" with
    | Some v -> as_string v
    | None -> err "wire binding is missing method"
  in
  let* wb_uri =
    match get "uri" with
    | None -> Ok (Ir.Wire_template [])
    | Some v -> decode_wire_value v
  in
  let* wb_bindings =
    match get "bindings" with
    | None -> Ok []
    | Some v -> decode_named_assoc decode_wire_part v
  in
  let* wb_response_bindings =
    match get "response_bindings" with
    | None -> Ok []
    | Some v -> decode_named_assoc decode_wire_response_part v
  in
  let* wb_success =
    match get "success" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result as_int xs
  in
  let* wb_request_headers =
    match get "request_headers" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_request_header xs
  in
  let* wb_query =
    match get "query" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_request_header xs
  in
  let opt_path k =
    match get k with
    | None -> Ok None
    | Some v ->
        let* p = decode_path v in
        Ok (Some p)
  in
  let* wb_endpoint =
    match get "endpoint" with
    | None -> Ok None
    | Some v ->
        let* w = decode_wire_value v in
        Ok (Some w)
  in
  let* wb_timeout = opt_path "timeout" in
  let* wb_retry = opt_path "retry" in
  Ok
    ({
       wb_method;
       wb_uri;
       wb_bindings;
       wb_response_bindings;
       wb_success;
       wb_endpoint;
       wb_request_headers;
       wb_query;
       wb_timeout;
       wb_retry;
     }
      : Ir.wire_binding)

let decode_wire_binding_opt = function
  | None | Some `Null -> Ok None
  | Some v ->
      let* b = decode_wire_binding v in
      Ok (Some b)
