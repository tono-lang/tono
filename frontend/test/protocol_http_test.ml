open Tono_frontend

(* ── resolve_op over hand-built IR ─────────────────────────────────────── *)

let trait id value : Ir.trait = { Ir.trait_id = id; value }

let member ?(traits = []) ?(target = Ir.Prim Ir.String) name : Ir.member =
  { Ir.name; target; required = true; default = None; constraints = []; traits }

let structure id members : Ir.shape =
  { Ir.id; kind = Ir.Structure { params = []; members }; traits = [] }

let error_shape id status code : Ir.shape =
  let code_trait =
    match code with
    | Some c -> [ trait "errorCode" (`List [ `String c ]) ]
    | None -> []
  in
  {
    Ir.id;
    kind = Ir.Structure { params = []; members = [] };
    traits = trait "status" (`List [ `Int status ]) :: code_trait;
  }

let op ?(traits = []) ?input ?output ?(errors = []) id : Ir.shape =
  { Ir.id; kind = Ir.Operation { input; output; errors }; traits }

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

let show_desc (d : Protocol_http.wire_descriptor) : string =
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
  let errors =
    List.map
      (fun (s, id, c) ->
        Printf.sprintf "%d:%s:%s" s id (Option.value ~default:"-" c))
      d.errors
  in
  String.concat "|"
    [
      d.http_method;
      d.uri;
      String.concat "," bindings;
      String.concat "," rbindings;
      String.concat "," success;
      String.concat "," errors;
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
  let nf = error_shape "nf" 404 None in
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
      ~errors:[ Ir.Ref ("nf", []) ]
  in
  let d = resolve [ req; resp; nf ] o in
  Alcotest.(check string)
    "descriptor"
    "GET|/things/{id}|id=label,limit=query(limit),auth=header(Authorization),note=body|trace<-header(X-Trace),code<-statusCode|200:resp|404:nf:-"
    (show_desc d)

(* An unmarked input defaults to body; success status defaults to 200. *)
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
    "body default + 200" "POST|/x|a=body,b=body||200:req|"
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
  (* No output type, but 200 is still the declared success status (no body). *)
  Alcotest.(check string)
    "payload" "PUT|/x|raw=payload||200:-|"
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
  Alcotest.(check string)
    "201" "POST|/x|||201:thing|"
    (show_desc (resolve [] o))

(* A bare @httpQuery/@httpHeader binds under the member's own name; an error
   whose shape lacks @status (or does not resolve) never enters the map. *)
let bare_bindings_and_dropped_errors () =
  let req =
    structure "req"
      [
        member "q" ~traits:[ trait "httpQuery" `Null ];
        member "h" ~traits:[ trait "httpHeader" `Null ];
      ]
  in
  let no_status = structure "no_status" [] in
  let o =
    op "act"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "get"); ("path", `String "/x") ]);
        ]
      ~input:(Ir.Ref ("req", []))
      ~errors:[ Ir.Ref ("no_status", []); Ir.Ref ("missing", []) ]
  in
  Alcotest.(check string)
    "bare names, no discriminable errors"
    "GET|/x|q=query(q),h=header(h)||200:-|"
    (show_desc (resolve [ req; no_status ] o))

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

(* resolve_module attaches the descriptor as a wire_descriptor trait on ops. *)
let module_attaches_trait () =
  let o =
    op "make"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "POST"); ("path", `String "/x") ]);
        ]
  in
  let m : Ir.module_ = { mod_name = "m"; shapes = []; operations = [ o ] } in
  let m' = Protocol_http.resolve_module m in
  let op' = List.hd m'.operations in
  Alcotest.(check bool)
    "has wire_descriptor" true
    (List.exists
       (fun (t : Ir.trait) -> t.trait_id = "wire_descriptor")
       op'.traits)

(* The JSON encoding exercises every part, response part, and error form. *)
let encode_covers_all_forms () =
  let req =
    structure "req"
      [
        member "id" ~traits:[ trait "httpLabel" `Null ];
        member "limit" ~traits:[ trait "httpQuery" (`List [ `String "limit" ]) ];
        member "auth"
          ~traits:[ trait "httpHeader" (`List [ `String "Authorization" ]) ];
        member "note";
        member "raw" ~traits:[ trait "httpPayload" `Null ];
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
  let coded = error_shape "coded" 402 (Some "declined") in
  let plain = error_shape "plain" 404 None in
  let o =
    op "act"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "post"); ("path", `String "/a/{id}") ]);
        ]
      ~input:(Ir.Ref ("req", []))
      ~output:(Ir.Ref ("resp", []))
      ~errors:[ Ir.Ref ("coded", []); Ir.Ref ("plain", []) ]
  in
  let json =
    Ir_json.to_canonical_string
      (Protocol_http.encode (resolve [ req; resp; coded; plain ] o))
  in
  let has sub =
    let ls = String.length json and lsub = String.length sub in
    let rec go i =
      if i + lsub > ls then false
      else if String.equal (String.sub json i lsub) sub then true
      else go (i + 1)
    in
    go 0
  in
  List.iter
    (fun sub -> Alcotest.(check bool) sub true (has sub))
    [
      {|"http_method":"POST"|};
      {|"uri":"/a/{id}"|};
      {|"kind":"label"|};
      {|"kind":"query","name":"limit"|};
      {|"kind":"header","name":"Authorization"|};
      {|"kind":"body"|};
      {|"kind":"payload"|};
      {|"kind":"statusCode"|};
      {|["auth"|};
      (* error with a code, and one without *)
      {|[402,"coded","declined"]|};
      {|[404,"plain",null]|};
    ]

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

(* A placeholder with no struct input to match against is still unmatched. *)
let placeholder_without_struct_input () =
  Alcotest.(check bool)
    "unmatched placeholder, primitive input" true
    (has "TC0019"
       "op get(string): string @http(method: \"get\", path: \"/x/{id}\")")

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
          Alcotest.test_case "bare bindings + dropped errors" `Quick
            bare_bindings_and_dropped_errors;
          Alcotest.test_case "no http no descriptor" `Quick
            no_http_no_descriptor;
          Alcotest.test_case "non-operation is none" `Quick
            non_operation_is_none;
          Alcotest.test_case "module attaches trait" `Quick
            module_attaches_trait;
          Alcotest.test_case "encode covers all forms" `Quick
            encode_covers_all_forms;
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
          Alcotest.test_case "placeholder without struct input" `Quick
            placeholder_without_struct_input;
          Alcotest.test_case "no http no checks" `Quick
            no_http_no_binding_checks;
        ] );
    ]
