open Tono_frontend

(* ── resolve_op over hand-built IR ─────────────────────────────────────── *)

let trait id value : Ir.trait = { Ir.trait_id = id; value }

let member ?(traits = []) ?(target = Ir.Prim Ir.String) name : Ir.member =
  { Ir.name; target; required = true; default = None; constraints = []; traits }

let structure id members : Ir.shape =
  { Ir.id; kind = Ir.Structure { params = []; members }; traits = [] }

let op ?(traits = []) ?pname ?input ?output ?(errors = []) id : Ir.shape =
  {
    Ir.id;
    kind =
      Ir.Operation { input; input_name = pname; output; errors; wire = None };
    traits;
  }

let field p : Ir.json =
  `Assoc [ ("field", `List (List.map (fun s -> `String s) p)) ]

let show_response_part : Protocol_http.response_part -> string = function
  | Response_header n -> Printf.sprintf "header(%s)" n
  | Response_status_code -> "statusCode"

let show_tref = function
  | Some (Ir.Ref (id, _)) -> id
  | Some _ -> "?"
  | None -> "-"

let rec show_template_part : Ir.template_part -> string = function
  | Ir.Tpl_lit s -> s
  | Ir.Tpl_field p -> Printf.sprintf "{.%s}" (String.concat "." p)
  | Ir.Tpl_param p -> Printf.sprintf "{.param.%s}" (String.concat "." p)
  | Ir.Tpl_input n -> Printf.sprintf "{%s}" n

and show_value_expr : Protocol_http.value_expr -> string = function
  | Protocol_http.Vlit (`String s) -> s
  | Protocol_http.Vlit _ -> "<lit>"
  | Protocol_http.Vfield p -> Printf.sprintf ".%s" (String.concat "." p)
  | Protocol_http.Vparam [] -> ".param"
  | Protocol_http.Vparam p -> Printf.sprintf ".param.%s" (String.concat "." p)
  | Protocol_http.Vtemplate parts ->
      String.concat "" (List.map show_template_part parts)
  | Protocol_http.Vctor fields ->
      let shown =
        List.map
          (fun (n, v) -> Printf.sprintf "%s:%s" n (show_value_expr v))
          fields
      in
      Printf.sprintf "{%s}" (String.concat "," shown)

let show_kv (parts, v) =
  Printf.sprintf "%s=%s"
    (String.concat "" (List.map show_template_part parts))
    (show_value_expr v)

let show_desc (d : Protocol_http.resolution) : string =
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
      show_value_expr d.uri;
      String.concat "," (List.map show_kv d.query);
      String.concat "," (List.map show_kv d.request_headers);
      (match d.body with None -> "-" | Some v -> show_value_expr v);
      String.concat "," rbindings;
      String.concat "," success;
    ]

(* A lookup over a fixed shape list, standing in for the module index. *)
let lookup shapes id = List.find_opt (fun (s : Ir.shape) -> s.id = id) shapes
let resolve shapes o = Option.get (Protocol_http.resolve_op (lookup shapes) o)

(* @query/@header/@body all declared at the op, plus the two response parts. *)
let resolve_full () =
  let resp =
    structure "resp"
      [
        member "trace"
          ~traits:[ trait "httpHeader" (`List [ `String "X-Trace" ]) ];
        member "code" ~traits:[ trait "httpResponseCode" `Null ];
      ]
  in
  let o =
    op "get_thing" ~pname:"ref"
      ~traits:
        [
          trait "http"
            (`Assoc
               [ ("method", `String "get"); ("path", `String "/things/{id}") ]);
          trait "query" (`List [ `String "limit"; field [ "ref"; "limit" ] ]);
          trait "header"
            (`List [ `String "Authorization"; field [ "ref"; "auth" ] ]);
        ]
      ~output:(Ir.Ref ("resp", []))
  in
  let d = resolve [ resp ] o in
  Alcotest.(check string)
    "descriptor"
    "GET|/things/{id}|limit=.param.limit|Authorization=.param.auth|-|trace<-header(X-Trace),code<-statusCode|"
    (show_desc d)

(* No @body means no body: it is never inferred from the input's shape. *)
let no_body_by_default () =
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
    "no body, no declared success code" "POST|/x|||-||"
    (show_desc (resolve [ req ] o))

(* @body(.param) sends the whole parameter as the body. *)
let body_whole_param () =
  let o =
    op "make" ~pname:"input"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "POST"); ("path", `String "/x") ]);
          trait "body" (`List [ field [ "input" ] ]);
        ]
  in
  Alcotest.(check string)
    "whole param" "POST|/x|||.param||"
    (show_desc (resolve [] o))

(* @body(.param.member) sends one member as the whole body. *)
let body_one_member () =
  let o =
    op "put" ~pname:"input"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "PUT"); ("path", `String "/x") ]);
          trait "body" (`List [ field [ "input"; "raw" ] ]);
        ]
  in
  Alcotest.(check string)
    "one member" "PUT|/x|||.param.raw||"
    (show_desc (resolve [] o))

(* @body's ctor mapper resolves each field to its own reference. *)
let body_ctor () =
  let o =
    op "put" ~pname:"input"
      ~traits:
        [
          trait "http"
            (`Assoc [ ("method", `String "PUT"); ("path", `String "/x") ]);
          trait "body"
            (`List
               [
                 `Assoc
                   [
                     ("ctor", `String "note_body");
                     ( "fields",
                       `Assoc
                         [
                           ("title", field [ "input"; "title" ]);
                           ("content", field [ "input"; "content" ]);
                         ] );
                   ];
               ]);
        ]
  in
  Alcotest.(check string)
    "ctor mapper" "PUT|/x|||{title:.param.title,content:.param.content}||"
    (show_desc (resolve [] o))

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
    "201" "POST|/x|||-||201:thing"
    (show_desc (resolve [] o))

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
    "200,207" "POST|/x|||-||200:thing,207:thing"
    (show_desc (resolve [] o))

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

(* ── @body shape diagnostics (spans, via source) ───────────────────────── *)

let codes src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  List.filter_map (fun (d : Diagnostic.t) -> d.code) tc

let has code src = List.mem code (codes src)

let body_whole_param_ok () =
  Alcotest.(check bool)
    "whole param body is fine" false
    (has "TC0044"
       "struct req { a: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(.input)")

let body_member_ok () =
  Alcotest.(check bool)
    "one member as body is fine" false
    (has "TC0044"
       "struct req { blob: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(.input.blob)")

let body_ctor_unknown_shape () =
  Alcotest.(check bool)
    "unknown ctor shape name" true
    (has "TC0044"
       "struct req { title: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(note_body { title: .input.title })")

let body_ctor_unknown_field () =
  Alcotest.(check bool)
    "unknown ctor field name" true
    (has "TC0044"
       "struct req { title: string }\n\
        struct note_body { title: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(note_body { headline: .input.title })")

let body_ctor_known_field_ok () =
  Alcotest.(check bool)
    "known ctor field is fine" false
    (has "TC0044"
       "struct req { title: string }\n\
        struct note_body { title: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(note_body { title: .input.title })")

let body_ctor_nested_ctor_rejected () =
  Alcotest.(check bool)
    "a ctor field cannot nest another ctor" true
    (has "TC0044"
       "struct req { title: string }\n\
        struct inner { x: string }\n\
        struct note_body { title: inner }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(note_body { title: inner { x: .input.title } })")

let body_ctor_field_input_placeholder_rejected () =
  Alcotest.(check bool)
    "a ctor field value rejects the legacy {name} input placeholder" true
    (has "TC0044"
       "struct req { title: string }\n\
        struct note_body { title: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(note_body { title: \"{title}\" })")

let body_two_positional_args_rejected () =
  Alcotest.(check bool)
    "@body takes exactly one argument" true
    (has "TC0044"
       "struct req { title: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(.input, .input)")

let body_bare_literal_rejected () =
  Alcotest.(check bool)
    "@body takes a reference or a ctor, not a bare literal" true
    (has "TC0044"
       "struct req { title: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(\"raw\")")

let duplicate_body_rejected () =
  Alcotest.(check bool)
    "at most one @body" true
    (has "TC0044"
       "struct req { title: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(.input) @body(.input)")

let body_requires_http () =
  Alcotest.(check bool)
    "@body without @http" true
    (has "TC0044"
       "struct req { title: string }\nop post(input: req): req @body(.input)")

(* A GET with no @body sends no body: nothing infers one from the input. *)
let no_body_when_undeclared () =
  Alcotest.(check bool)
    "no @body, no diagnostic" false
    (has "TC0044"
       "struct req { title: string }\n\
        op get(input: req): req @http(method: \"get\", path: \"/x\")")

let body_ctor_duplicate_field_rejected () =
  Alcotest.(check bool)
    "a ctor cannot map the same field twice" true
    (has "TC0044"
       "struct req { title: string }\n\
        struct note_body { title: string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @body(note_body { title: .input.title, title: .input.title })")

(* ── @http(code:) shape diagnostics ────────────────────────────────────── *)

let empty_code_list_rejected () =
  Alcotest.(check bool)
    "@http(code: []) is rejected" true
    (has "TC0068"
       "op post(): void @http(method: \"post\", path: \"/x\", code: [])")

let non_int_code_element_rejected () =
  Alcotest.(check bool)
    "@http(code: [200, \"x\"]) is rejected" true
    (has "TC0068"
       "op post(): void @http(method: \"post\", path: \"/x\", code: [200, \
        \"x\"])")

let non_int_code_scalar_rejected () =
  Alcotest.(check bool)
    "@http(code: \"x\") is rejected" true
    (has "TC0068"
       "op post(): void @http(method: \"post\", path: \"/x\", code: \"x\")")

let valid_code_forms_accepted () =
  Alcotest.(check bool)
    "@http(code: 201) is fine" false
    (has "TC0068"
       "op post(): void @http(method: \"post\", path: \"/x\", code: 201)");
  Alcotest.(check bool)
    "@http(code: [200, 207]) is fine" false
    (has "TC0068"
       "op post(): void @http(method: \"post\", path: \"/x\", code: [200, 207])")

(* ── @query/@header value type diagnostics ─────────────────────────────── *)

let query_map_value_rejected () =
  Alcotest.(check bool)
    "@query value cannot be a map" true
    (has "TC0021"
       "struct req { tags: map[string]string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @query(\"tags\", .input.tags)")

let header_list_value_rejected () =
  Alcotest.(check bool)
    "@header value cannot be a list" true
    (has "TC0021"
       "struct req { tags: []string }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @header(\"X-Tags\", .input.tags)")

let query_scalar_value_ok () =
  Alcotest.(check bool)
    "@query value is fine when scalar" false
    (has "TC0021"
       "struct req { limit: i32 }\n\
        op post(input: req): req @http(method: \"post\", path: \"/x\") \
        @query(\"limit\", .input.limit)")

let () =
  Alcotest.run "protocol_http"
    [
      ( "resolve",
        [
          Alcotest.test_case "resolve full" `Quick resolve_full;
          Alcotest.test_case "no body by default" `Quick no_body_by_default;
          Alcotest.test_case "body whole param" `Quick body_whole_param;
          Alcotest.test_case "body one member" `Quick body_one_member;
          Alcotest.test_case "body ctor" `Quick body_ctor;
          Alcotest.test_case "success code override" `Quick
            success_code_override;
          Alcotest.test_case "success code list" `Quick success_code_list;
          Alcotest.test_case "no http no descriptor" `Quick
            no_http_no_descriptor;
          Alcotest.test_case "non-operation is none" `Quick
            non_operation_is_none;
          Alcotest.test_case "module attaches wire" `Quick module_attaches_wire;
        ] );
      ( "body_shapes",
        [
          Alcotest.test_case "whole param body ok" `Quick body_whole_param_ok;
          Alcotest.test_case "member body ok" `Quick body_member_ok;
          Alcotest.test_case "ctor unknown shape" `Quick body_ctor_unknown_shape;
          Alcotest.test_case "ctor unknown field" `Quick body_ctor_unknown_field;
          Alcotest.test_case "ctor known field ok" `Quick
            body_ctor_known_field_ok;
          Alcotest.test_case "ctor nested ctor rejected" `Quick
            body_ctor_nested_ctor_rejected;
          Alcotest.test_case "ctor field input placeholder rejected" `Quick
            body_ctor_field_input_placeholder_rejected;
          Alcotest.test_case "two positional args rejected" `Quick
            body_two_positional_args_rejected;
          Alcotest.test_case "bare literal rejected" `Quick
            body_bare_literal_rejected;
          Alcotest.test_case "duplicate body rejected" `Quick
            duplicate_body_rejected;
          Alcotest.test_case "body requires http" `Quick body_requires_http;
          Alcotest.test_case "no body when undeclared" `Quick
            no_body_when_undeclared;
          Alcotest.test_case "ctor duplicate field rejected" `Quick
            body_ctor_duplicate_field_rejected;
        ] );
      ( "http_code_shapes",
        [
          Alcotest.test_case "empty code list rejected" `Quick
            empty_code_list_rejected;
          Alcotest.test_case "non-int code element rejected" `Quick
            non_int_code_element_rejected;
          Alcotest.test_case "non-int code scalar rejected" `Quick
            non_int_code_scalar_rejected;
          Alcotest.test_case "valid code forms accepted" `Quick
            valid_code_forms_accepted;
        ] );
      ( "http_kv_value_types",
        [
          Alcotest.test_case "query map value rejected" `Quick
            query_map_value_rejected;
          Alcotest.test_case "header list value rejected" `Quick
            header_list_value_rejected;
          Alcotest.test_case "query scalar value ok" `Quick
            query_scalar_value_ok;
        ] );
    ]
