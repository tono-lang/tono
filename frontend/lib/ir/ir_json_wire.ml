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

let encode_wire_response_part : Ir.wire_response_part -> Ir.json = function
  | Ir.Wire_response_header name ->
      `Assoc [ ("kind", `String "header"); ("name", `String name) ]
  | Ir.Wire_response_status_code -> `Assoc [ ("kind", `String "statusCode") ]

let rec encode_wire_call_arg : Ir.wire_call_arg -> Ir.json = function
  | Ir.Wca_field p -> `Assoc [ ("field", encode_path p) ]
  | Ir.Wca_param p -> `Assoc [ ("param", encode_path p) ]
  | Ir.Wca_lit v -> `Assoc [ ("lit", v) ]
  | Ir.Wca_ctor fields ->
      `Assoc
        [
          ( "ctor",
            `List
              (List.map
                 (fun (n, v) -> `List [ `String n; encode_wire_call_arg v ])
                 fields) );
        ]
  | Ir.Wca_request -> `String "request"

let encode_wire_call (c : Ir.wire_call) : Ir.json =
  `Assoc
    [
      ("ns", `String c.wcl_ns);
      ("fn", `String c.wcl_fn);
      ("args", `List (List.map encode_wire_call_arg c.wcl_args));
    ]

let rec encode_wire_value : Ir.wire_value -> Ir.json = function
  | Ir.Wire_lit j -> `Assoc [ ("lit", j) ]
  | Ir.Wire_field p -> `Assoc [ ("field", encode_path p) ]
  | Ir.Wire_param p -> `Assoc [ ("param", encode_path p) ]
  | Ir.Wire_template parts ->
      `Assoc [ ("template", `List (List.map encode_template_part parts)) ]
  | Ir.Wire_object fields ->
      `Assoc
        [
          ( "object",
            `List
              (List.map
                 (fun (n, v) -> `List [ `String n; encode_wire_value v ])
                 fields) );
        ]
  | Ir.Wire_call c -> `Assoc [ ("call", encode_wire_call c) ]

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
       ( "response_bindings",
         encode_named_assoc encode_wire_response_part b.wb_response_bindings );
       ("success", `List (List.map (fun s -> `Int s) b.wb_success));
       ("request_headers", `List (List.map request_header b.wb_request_headers));
       ("query", `List (List.map request_header b.wb_query));
     ]
    @ (match b.wb_body with
      | None -> []
      | Some v -> [ ("body", encode_wire_value v) ])
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

let rec decode_object_field j =
  match j with
  | `List [ `String n; v ] ->
      let* w = decode_wire_value v in
      Ok (n, w)
  | _ -> err "wire object field must be a [name, value] pair"

and decode_wire_call_arg_field j =
  match j with
  | `List [ `String n; v ] ->
      let* w = decode_wire_call_arg v in
      Ok (n, w)
  | _ -> err "wire call ctor field must be a [name, value] pair"

and decode_wire_call_arg j =
  match j with
  | `String "request" -> Ok Ir.Wca_request
  | _ -> (
      let* kvs = as_assoc j in
      match kvs with
      | [ ("field", v) ] ->
          let* p = decode_path v in
          Ok (Ir.Wca_field p)
      | [ ("param", v) ] ->
          let* p = decode_path v in
          Ok (Ir.Wca_param p)
      | [ ("lit", v) ] -> Ok (Ir.Wca_lit v)
      | [ ("ctor", v) ] ->
          let* xs = as_list v in
          let* fields = map_result decode_wire_call_arg_field xs in
          Ok (Ir.Wca_ctor fields)
      | _ ->
          err
            "wire call argument must be \"request\" or a single field, \
             param, lit, or ctor key")

and decode_wire_call j =
  let* kvs = as_assoc j in
  let* wcl_ns =
    match List.assoc_opt "ns" kvs with
    | Some v -> as_string v
    | None -> err "wire call is missing ns"
  in
  let* wcl_fn =
    match List.assoc_opt "fn" kvs with
    | Some v -> as_string v
    | None -> err "wire call is missing fn"
  in
  let* wcl_args =
    match List.assoc_opt "args" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_wire_call_arg xs
  in
  Ok ({ wcl_ns; wcl_fn; wcl_args } : Ir.wire_call)

and decode_wire_value j =
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
  | [ ("object", v) ] ->
      let* xs = as_list v in
      let* fields = map_result decode_object_field xs in
      Ok (Ir.Wire_object fields)
  | [ ("call", v) ] ->
      let* c = decode_wire_call v in
      Ok (Ir.Wire_call c)
  | _ ->
      err
        "wire value must be a single lit, field, param, template, object, or \
         call key"

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
  let* wb_body =
    match get "body" with
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
       wb_body;
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
