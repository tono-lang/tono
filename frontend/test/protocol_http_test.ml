open Tono_frontend

(* ── resolve_op over hand-built IR ─────────────────────────────────────── *)

let trait id value : Ir.trait = { Ir.trait_id = id; value }

let member ?(traits = []) ?(target = Ir.Prim Ir.String) name : Ir.member =
  { Ir.name; target; required = true; default = None; constraints = []; traits }

let structure id members : Ir.shape =
  { Ir.id; kind = Ir.Structure { params = []; members }; traits = [] }

let op ?(traits = []) ?input ?output ?(errors = []) id : Ir.shape =
  { Ir.id; kind = Ir.Operation { input; output; errors; wire = None }; traits }

(* A stable rendering of a descriptor, doubling as the snapshot format. *)
let show_part : Protocol_http.part -> string = function
  | Label -> "label"
  | Query n -> Printf.sprintf "query(%s)" n
  | Header n -> Printf.sprintf "header(%s)" n
  | Body -> "body"
  | Payload -> "payload"

let show_response_part : Protocol_http.response_part -> string = function
  | Response_header n -> Printf.sprintf "header(%s)" n
  | Response_status_code -> "statusCode"

let show_tref = function
  | Some (Ir.Ref (id, _)) -> id
  | Some _ -> "?"
  | None -> "-"

let show_desc (d : Protocol_http.resolution) : string =
  let bindings =
    List.map (fun (n, p) -> Printf.sprintf "%s=%s" n (show_part p)) d.bindings
  in
  let rbindings =
    List.map
      (fun (n, p) -> Printf.sprintf "%s<-%s" n (show_response_part p))
      d.response_bindings
  in
  let success =
    List.map (fun (s, t) -> Printf.sprintf "%d:%s" s (show_tref t)) d.success
  in
  String.concat "|"
    [
      d.http_method;
      d.uri;
      String.concat "," bindings;
      String.concat "," rbindings;
      String.concat "," success;
    ]

(* A lookup over a fixed shape list, standing in for the module index. *)
let lookup shapes id = List.find_opt (fun (s : Ir.shape) -> s.id = id) shapes
let resolve shapes o = Option.get (Protocol_http.resolve_op (lookup shapes) o)

(* Every part variant plus the two response parts, in one operation. *)
let all_parts () =
  let req =
    structure "req"
      [
        member "id" ~traits:[ trait "httpLabel" `Null ];
        member "limit" ~traits:[ trait "httpQuery" (`List [ `String "limit" ]) ];
        member "auth"
          ~traits:[ trait "httpHeader" (`List [ `String "Authorization" ]) ];
        member "note";
      ]
  in
  let resp =
    structure "resp"
      [
        member "trace"
          ~traits:[ trait "httpHeader" (`List [ `String "X-Trace" ]) ];
        member "code" ~traits:[ trait "httpResponseCode" `Null ];
      ]
  in
  let o =
    op "get_thing"
      ~traits:
        [
          trait "http"
            (`Assoc
               [ ("method", `String "get"); ("path", `String "/things/{id}") ]);
        ]
      ~input:(Ir.Ref ("req", []))
      ~output:(Ir.Ref ("resp", []))
  in
  let d = resolve [ req; resp ] o in
  Alcotest.(check string)
    "descriptor"
    "GET|/things/{id}|id=label,limit=query(limit),auth=header(Authorization),note=body|trace<-header(X-Trace),code<-statusCode|"
    (show_desc d)

(* An unmarked input defaults to body; no @http(code:) leaves success empty
   (every emitter falls back to the 2xx-range convention). *)
let body_default () =
  let req = structure "req" [ member "a"; member "b" ] in
  let o =
    op "make"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "POST"); ("path", `String "/x") ]);
        ]
      ~input:(Ir.Ref ("req", []))
      ~output:(Ir.Ref ("req", []))
  in
  Alcotest.(check string)
    "body default, no declared success code" "POST|/x|a=body,b=body||"
    (show_desc (resolve [ req ] o))

(* An explicit @httpPayload member occupies the whole body. *)
let payload_whole_body () =
  let req =
    structure "req" [ member "raw" ~traits:[ trait "httpPayload" `Null ] ]
  in
  let o =
    op "put"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "PUT"); ("path", `String "/x") ]);
        ]
      ~input:(Ir.Ref ("req", []))
  in
  (* No output type and no declared success code either. *)
  Alcotest.(check string)
    "payload" "PUT|/x|raw=payload||"
    (show_desc (resolve [ req ] o))

(* A success code override rides @http(code:). *)
let success_code_override () =
  let o =
    op "create"
      ~traits:
        [
          trait "http"
            (`Assoc
               [
                 ("method", `String "POST");
                 ("path", `String "/x");
                 ("code", `Int 201);
               ]);
        ]
      ~output:(Ir.Ref ("thing", []))
  in
  Alcotest.(check string) "201" "POST|/x|||201:thing" (show_desc (resolve [] o))

(* A list of success codes rides @http(code: [...]) too. *)
let success_code_list () =
  let o =
    op "bulk_create"
      ~traits:
        [
          trait "http"
            (`Assoc
               [
                 ("method", `String "POST");
                 ("path", `String "/x");
                 ("code", `List [ `Int 200; `Int 207 ]);
               ]);
        ]
      ~output:(Ir.Ref ("thing", []))
  in
  Alcotest.(check string)
    "200,207" "POST|/x|||200:thing,207:thing"
    (show_desc (resolve [] o))

(* A bare @httpQuery/@httpHeader binds under the member's own name. *)
let bare_bindings () =
  let req =
    structure "req"
      [
        member "q" ~traits:[ trait "httpQuery" `Null ];
        member "h" ~traits:[ trait "httpHeader" `Null ];
      ]
  in
  let o =
    op "act"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "get"); ("path", `String "/x") ]);
        ]
      ~input:(Ir.Ref ("req", []))
  in
  Alcotest.(check string)
    "bare names" "GET|/x|q=query(q),h=header(h)||"
    (show_desc (resolve [ req ] o))

(* An operation with no @http trait carries no descriptor. *)
let no_http_no_descriptor () =
  let o = op "local" ~input:(Ir.Ref ("req", [])) in
  Alcotest.(check bool)
    "none" true
    (Protocol_http.resolve_op (lookup []) o = None)

(* Resolving a non-operation shape yields no descriptor. *)
let non_operation_is_none () =
  let s = structure "req" [ member "a" ] in
  Alcotest.(check bool)
    "structure -> none" true
    (Protocol_http.resolve_op (lookup []) s = None)

(* resolve_module attaches the resolution as a typed wire field on ops. *)
let module_attaches_wire () =
  let o =
    op "make"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "POST"); ("path", `String "/x") ]);
        ]
  in
  let m : Ir.module_ =
    {
      mod_name = "m";
      shapes = [];
      operations = [ o ];
      extensions = [];
      tests = [];
    }
  in
  let m' = Protocol_http.resolve_module m in
  let op' = List.hd m'.operations in
  match op'.kind with
  | Ir.Operation { wire = Some wb; _ } ->
      Alcotest.(check string) "method" "POST" wb.Ir.wb_method
  | _ -> Alcotest.fail "op has no wire binding"

(* ── Check_http negatives (spans, via source) ──────────────────────────── *)

let codes src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  List.filter_map (fun (d : Diagnostic.t) -> d.code) tc

let has code src = List.mem code (codes src)

let label_matches_ok () =
  Alcotest.(check bool)
    "label matches placeholder" false
    (has "TC0019"
       "struct req { id: string @httpLabel }\n\
        op get(req): req @http(method: \"get\", path: \"/x/{id}\")")

let placeholder_without_label () =
  Alcotest.(check bool)
    "unmatched placeholder" true
    (has "TC0019"
       "struct req { other: string }\n\
        op get(req): req @http(method: \"get\", path: \"/x/{id}\")")

let label_without_placeholder () =
  Alcotest.(check bool)
    "unmatched label" true
    (has "TC0019"
       "struct req { id: string @httpLabel }\n\
        op get(req): req @http(method: \"get\", path: \"/x\")")

let payload_with_body_conflicts () =
  Alcotest.(check bool)
    "payload/body conflict" true
    (has "TC0020"
       "struct req { raw: string @httpPayload, extra: string }\n\
        op put(req): req @http(method: \"put\", path: \"/x\")")

let two_payloads () =
  Alcotest.(check bool)
    "second payload" true
    (has "TC0020"
       "struct req { a: string @httpPayload, b: string @httpPayload }\n\
        op put(req): req @http(method: \"put\", path: \"/x\")")

let map_in_query_rejected () =
  Alcotest.(check bool)
    "map in query" true
    (has "TC0021"
       "struct req { m: map[string]string @httpQuery(\"m\") }\n\
        op get(req): req @http(method: \"get\", path: \"/x\")")

let map_in_header_rejected () =
  Alcotest.(check bool)
    "map in header" true
    (has "TC0021"
       "struct req { m: map[string]string @httpHeader(\"M\") }\n\
        op get(req): req @http(method: \"get\", path: \"/x\")")

(* A nullable map member still counts as a map for the query/header ban. *)
let nullable_map_in_query_rejected () =
  Alcotest.(check bool)
    "nullable map in query" true
    (has "TC0021"
       "struct req { m: map[string]string? @httpQuery(\"m\") }\n\
        op get(req): req @http(method: \"get\", path: \"/x\")")

(* A nullable @httpLabel member is rejected: a path parameter is always present. *)
let nullable_label_rejected () =
  Alcotest.(check bool)
    "nullable label" true
    (has "TC0022"
       "struct req { id: string? @httpLabel }\n\
        op get(req): req @http(method: \"get\", path: \"/x/{id}\")")

(* A placeholder with no struct input to match against is still unmatched. *)
let placeholder_without_struct_input () =
  Alcotest.(check bool)
    "unmatched placeholder, primitive input" true
    (has "TC0019"
       "op get(string): string @http(method: \"get\", path: \"/x/{id}\")")

(* An empty code: list is malformed, not "no code declared". *)
let empty_code_list_rejected () =
  Alcotest.(check bool)
    "empty code list" true
    (has "TC0068"
       "struct req { }\n\
        op post(req): req @http(method: \"post\", path: \"/x\", code: [])")

(* A code: list with a non-int element is malformed the same way. *)
let non_int_code_element_rejected () =
  Alcotest.(check bool)
    "non-int code element" true
    (has "TC0068"
       "struct req { }\n\
        op post(req): req @http(method: \"post\", path: \"/x\", code: [200, \
        \"x\"])")

(* A non-int scalar code: is malformed too. *)
let non_int_code_scalar_rejected () =
  Alcotest.(check bool)
    "non-int code scalar" true
    (has "TC0068"
       "struct req { }\n\
        op post(req): req @http(method: \"post\", path: \"/x\", code: \"x\")")

let valid_code_forms_accepted () =
  Alcotest.(check bool)
    "int and int-list code are both fine" false
    (has "TC0068"
       "struct req { }\n\
        op post(req): req @http(method: \"post\", path: \"/x\", code: [200, \
        207])")

let no_http_no_binding_checks () =
  Alcotest.(check (list string))
    "no http, no binding diagnostics" []
    (List.filter
       (fun c -> c = "TC0019" || c = "TC0020" || c = "TC0021")
       (codes
          "struct req { m: map[string]string @httpQuery(\"m\") }\n\
           op get(req): req"))

let () =
  Alcotest.run "protocol_http"
    [
      ( "resolve",
        [
          Alcotest.test_case "all parts" `Quick all_parts;
          Alcotest.test_case "body default" `Quick body_default;
          Alcotest.test_case "payload whole body" `Quick payload_whole_body;
          Alcotest.test_case "success code override" `Quick
            success_code_override;
          Alcotest.test_case "success code list" `Quick success_code_list;
          Alcotest.test_case "bare bindings" `Quick bare_bindings;
          Alcotest.test_case "no http no descriptor" `Quick
            no_http_no_descriptor;
          Alcotest.test_case "non-operation is none" `Quick
            non_operation_is_none;
          Alcotest.test_case "module attaches wire" `Quick module_attaches_wire;
        ] );
      ( "check_http",
        [
          Alcotest.test_case "label matches ok" `Quick label_matches_ok;
          Alcotest.test_case "placeholder without label" `Quick
            placeholder_without_label;
          Alcotest.test_case "label without placeholder" `Quick
            label_without_placeholder;
          Alcotest.test_case "payload/body conflict" `Quick
            payload_with_body_conflicts;
          Alcotest.test_case "two payloads" `Quick two_payloads;
          Alcotest.test_case "map in query" `Quick map_in_query_rejected;
          Alcotest.test_case "map in header" `Quick map_in_header_rejected;
          Alcotest.test_case "nullable map in query" `Quick
            nullable_map_in_query_rejected;
          Alcotest.test_case "nullable label rejected" `Quick
            nullable_label_rejected;
          Alcotest.test_case "placeholder without struct input" `Quick
            placeholder_without_struct_input;
          Alcotest.test_case "empty code list rejected" `Quick
            empty_code_list_rejected;
          Alcotest.test_case "non-int code element rejected" `Quick
            non_int_code_element_rejected;
          Alcotest.test_case "non-int code scalar rejected" `Quick
            non_int_code_scalar_rejected;
          Alcotest.test_case "valid code forms accepted" `Quick
            valid_code_forms_accepted;
          Alcotest.test_case "no http no checks" `Quick
            no_http_no_binding_checks;
        ] );
    ]
