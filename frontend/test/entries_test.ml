open Tono_frontend

(* Parse + lower + typecheck a snippet, returning the typecheck diagnostics. *)
let check src =
  let file, pd = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length pd);
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

let contains hay needle =
  let hn = String.length hay and nn = String.length needle in
  let rec loop i =
    if i + nn > hn then false
    else if String.sub hay i nn = needle then true
    else loop (i + 1)
  in
  nn = 0 || loop 0

let compile src =
  let file, pd = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length pd);
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  Alcotest.(check int) "no lowering diagnostics" 0 (List.length !diags);
  m

let shape_by_id (m : Ir.module_) id =
  match List.find_opt (fun (s : Ir.shape) -> s.id = id) m.shapes with
  | Some s -> s
  | None -> Alcotest.failf "shape %s not found" id

(* The canonical entry-model client: every source kind, derivation,
   selection, composition, and an op consuming entry fields through its
   traits. *)
let canonical_client =
  {|
struct conf {
  api_key: string @env("API_KEY")
  region: string @env("REGION") @default("us")
}

pub struct client {
  api_key: string @arg
  client_name: string @with @default("demo")
  client_key: string @format("{.client_name}") @str::trim @str::upper_snake
  endpoint_env: string @format("ENDPOINT_{.client_key}_V2")
  endpoint_version: string @env("ENDPOINT_VERSION") @default("v2")
  endpoint_v1: string @env("ENDPOINT")
  endpoint_v2: string @env(.endpoint_env)
  endpoint: string = match .endpoint_version {
    "v1" => .endpoint_v1
    "v2" => .endpoint_v2
    _ => .endpoint_v2
  }
  timeout: duration @with @default("10s")
  max_retries: i32 @with @default(3)
  settings: conf @bind(api_key, .api_key)

  op save_note(note): note
    @http(method: "POST", path: "/notes/{id}", endpoint: .endpoint)
    @header("X-Client-Name", .client_name)
    @timeout(.timeout)
    @retry(.max_retries)
    @errors(overloaded)
  op ping()
}

ext impl ping {
  go: "ext/go/ping.go#Ping"
}

struct note {
  id: string @httpLabel
  body: string
}

@status(529)
@errorCode("overloaded")
@retryable
struct overloaded {
  message: string
}
|}

(* ── Parser + typecheck: the canonical forms are accepted ──────────────── *)

let canonical_accepted () =
  Alcotest.(check (list string)) "no diagnostics" [] (codes canonical_client)

let loose_ops_untouched () =
  Alcotest.(check (list string))
    "plain specs stay clean" []
    (codes
       "struct charge { amount: i64 }\n\
        op create(charge): charge @http(method: \"POST\", path: \"/c\") @async")

(* ── Lowering: the IR entry surface ────────────────────────────────────── *)

let lowered_entry () =
  let m = compile canonical_client in
  let client = shape_by_id m "m#client" in
  match client.kind with
  | Ir.Entry { fields; operations } ->
      Alcotest.(check int) "fields" 11 (List.length fields);
      Alcotest.(check int) "nested ops" 2 (List.length operations);
      let field name =
        match
          List.find_opt (fun (f : Ir.entry_field) -> f.ef_name = name) fields
        with
        | Some f -> f
        | None -> Alcotest.failf "field %s not found" name
      in
      (* sources, in declared fallback order *)
      Alcotest.(check bool)
        "api_key is @arg" true
        ((field "api_key").ef_sources = [ Ir.Arg ]);
      Alcotest.(check bool)
        "client_name stacks with+default" true
        ((field "client_name").ef_sources
        = [ Ir.With; Ir.Default (`String "demo") ]);
      Alcotest.(check bool)
        "env by name" true
        ((field "endpoint_v1").ef_sources = [ Ir.Env (Ir.Env_name "ENDPOINT") ]);
      Alcotest.(check bool)
        "env by field ref" true
        ((field "endpoint_v2").ef_sources
        = [ Ir.Env (Ir.Env_field [ "endpoint_env" ]) ]);
      (* derivation *)
      Alcotest.(check bool)
        "format parses placeholders" true
        ((field "endpoint_env").ef_format
        = Some
            [
              Ir.Tpl_lit "ENDPOINT_";
              Ir.Tpl_field [ "client_key" ];
              Ir.Tpl_lit "_V2";
            ]);
      Alcotest.(check (list string))
        "transform pipeline order" [ "trim"; "upper_snake" ]
        (field "client_key").ef_transforms;
      (* selection *)
      (match (field "endpoint").ef_select with
      | Some { subject; arms } ->
          Alcotest.(check (list string))
            "subject" [ "endpoint_version" ] subject;
          Alcotest.(check int) "arms" 3 (List.length arms);
          Alcotest.(check bool)
            "wildcard last" true
            ((List.nth arms 2).arm_pattern = None)
      | None -> Alcotest.fail "endpoint has no select");
      (* composition *)
      Alcotest.(check bool)
        "bind target and source" true
        ((field "settings").ef_binds
        = [ { Ir.bind_field = "api_key"; bind_source = [ "api_key" ] } ]);
      (* nested op identity and trait refs *)
      let op = List.hd operations in
      Alcotest.(check string) "entry-scoped op id" "m#client.save_note" op.id;
      let http =
        match
          List.find_opt (fun (t : Ir.trait) -> t.trait_id = "http") op.traits
        with
        | Some t -> t
        | None -> Alcotest.fail "no @http on nested op"
      in
      Alcotest.(check bool)
        "endpoint lowers as a structured ref" true
        (match http.value with
        | `Assoc kvs ->
            List.assoc_opt "endpoint" kvs
            = Some (`Assoc [ ("field", `List [ `String "endpoint" ]) ])
        | _ -> false)
  | _ -> Alcotest.fail "client did not lower to an entry"

let lowered_config () =
  let m = compile canonical_client in
  match (shape_by_id m "m#conf").kind with
  | Ir.Config { fields } ->
      Alcotest.(check int) "config fields" 2 (List.length fields)
  | _ -> Alcotest.fail "conf did not lower to a config"

(* A struct an entry composes purely by @bind (no sources of its own) is still
   a config, not a wire structure. *)
let bind_only_config () =
  let m =
    compile
      "struct conf { api_key: string }\n\
       pub struct client {\n\
      \  api_key: string @arg\n\
      \  settings: conf @bind(api_key, .api_key)\n\
      \  op ping()\n\
       }"
  in
  match (shape_by_id m "m#conf").kind with
  | Ir.Config _ -> ()
  | _ -> Alcotest.fail "bind-only conf did not classify as config"

(* ── IR round-trip: encode(model) decodes back to the same model ───────── *)

let ir_roundtrip () =
  let m = compile canonical_client in
  let model : Ir.model =
    { tono_ir_version = Ir_json.current_ir_version; modules = [ m ] }
  in
  let json = Ir_json.encode_model model in
  match Ir_json.decode_model json with
  | Error e -> Alcotest.failf "decode failed: %s" e
  | Ok decoded ->
      Alcotest.(check string)
        "encode . decode = id"
        (Ir_json.to_canonical_string json)
        (Ir_json.to_canonical_string (Ir_json.encode_model decoded))

let version_is_9 () =
  Alcotest.(check int) "wire version" 9 Ir_json.current_ir_version

(* ── fmt: the new forms print and re-parse to the same text ────────────── *)

let fmt_roundtrip () =
  let file, pd = Parser.parse canonical_client in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length pd);
  let printed = Printer.print_file file in
  let reparsed, pd2 = Parser.parse printed in
  Alcotest.(check int) "printed form re-parses" 0 (List.length pd2);
  Alcotest.(check string)
    "printing is a fixpoint" printed
    (Printer.print_file reparsed)

(* ── Protocol: entry refs land in the typed wire binding ────────────────── *)

let descriptor_carries_refs () =
  let m = Protocol_http.resolve_module (compile canonical_client) in
  let client = shape_by_id m "m#client" in
  match client.kind with
  | Ir.Entry { operations; _ } ->
      let op = List.hd operations in
      let wb =
        match op.kind with
        | Ir.Operation { wire = Some wb; _ } -> wb
        | _ -> Alcotest.fail "nested op has no wire binding"
      in
      Alcotest.(check bool)
        "endpoint ref" true
        (wb.Ir.wb_endpoint = Some [ "endpoint" ]);
      Alcotest.(check bool)
        "timeout is a plain entry-field ref" true
        (wb.Ir.wb_timeout = Some [ "timeout" ]);
      Alcotest.(check bool)
        "retry is a plain entry-field ref" true
        (wb.Ir.wb_retry = Some [ "max_retries" ]);
      (* A declared error's (status, @errorCode, @retryable) is the generated
         SDK's own decode<Op>Error + .retryable() pair to own; wire_binding
         has no field for it, so there is nothing to assert here. *)
      Alcotest.(check bool)
        "one declared header" true
        (match wb.Ir.wb_request_headers with
        | [ (_key, Ir.Wire_field [ f ]) ] -> String.equal f "client_name"
        | _ -> false);
      Alcotest.(check bool)
        "uri keeps both placeholder scopes as typed parts" true
        (wb.Ir.wb_uri = [ Ir.Tpl_lit "/notes/"; Ir.Tpl_input "id" ])
  | _ -> Alcotest.fail "client is not an entry"

let loose_descriptor_has_no_entry_fields () =
  let m =
    Protocol_http.resolve_module
      (compile
         "struct w { x: string }\n\
          op o(w): w @http(method: \"GET\", path: \"/w\")")
  in
  let op = List.hd m.operations in
  let wb =
    match op.kind with
    | Ir.Operation { wire = Some wb; _ } -> wb
    | _ -> Alcotest.fail "loose op has no wire binding"
  in
  Alcotest.(check bool)
    "no entry-scoped fields" true
    (wb.Ir.wb_endpoint = None && wb.Ir.wb_timeout = None
    && wb.Ir.wb_retry = None && wb.Ir.wb_request_headers = [])

(* A second, dedicated snippet (kept separate from [canonical_client] so its
   many other tests stay untouched): query/payload input bindings and
   header/status-code response bindings, which [canonical_client] never
   exercises. Covers [Protocol_http.to_ir_binding]'s full match surface
   together with [descriptor_carries_refs]'s label/header/endpoint/timeout/
   retry coverage above. *)
let wire_probe_client =
  {|
struct probe_body {
  note: string
}

struct probe_input {
  id: string @httpLabel
  filter: string @httpQuery("q")
  x_key: string @httpHeader("X-Key")
  body: probe_body @httpPayload
}

struct probe_output {
  value: string
  trace_id: string @httpHeader("X-Trace-Id")
  code: i32 @httpResponseCode
}

pub struct probe_client {
  api_key: string @arg
  endpoint: string @env("ENDPOINT")

  op probe(probe_input): probe_output
    @http(method: "POST", path: "/probe/{id}", endpoint: .endpoint)
    @header("X-Client", .api_key)
    @header("X-Combo", "v-{.api_key}")
}
|}

let wire_field_covers_query_payload_and_response_bindings () =
  let m = Protocol_http.resolve_module (compile wire_probe_client) in
  let client = shape_by_id m "m#probe_client" in
  match client.kind with
  | Ir.Entry { operations; _ } -> (
      let op = List.hd operations in
      match op.kind with
      | Ir.Operation { wire = Some w; _ } ->
          Alcotest.(check string) "method" "POST" w.wb_method;
          Alcotest.(check (list string))
            "uri keeps the label placeholder" [ "/probe/"; "{id}" ]
            (List.filter_map
               (function
                 | Ir.Tpl_lit s -> Some s
                 | Ir.Tpl_input n -> Some (Printf.sprintf "{%s}" n)
                 | Ir.Tpl_field _ -> None)
               w.wb_uri);
          let binding name = List.assoc_opt name w.wb_bindings in
          Alcotest.(check bool)
            "id is a label" true
            (binding "id" = Some Ir.Wire_label);
          Alcotest.(check bool)
            "filter is a named query" true
            (binding "filter" = Some (Ir.Wire_query "q"));
          Alcotest.(check bool)
            "x_key is a named header" true
            (binding "x_key" = Some (Ir.Wire_header "X-Key"));
          Alcotest.(check bool)
            "body is the whole payload" true
            (binding "body" = Some Ir.Wire_payload);
          let response_binding name =
            List.assoc_opt name w.wb_response_bindings
          in
          Alcotest.(check bool)
            "trace_id is a response header" true
            (response_binding "trace_id"
            = Some (Ir.Wire_response_header "X-Trace-Id"));
          Alcotest.(check bool)
            "code is the response status code" true
            (response_binding "code" = Some Ir.Wire_response_status_code);
          Alcotest.(check bool)
            "single success code" true (w.wb_success = [ 200 ]);
          Alcotest.(check bool)
            "endpoint ref" true
            (w.wb_endpoint = Some [ "endpoint" ]);
          Alcotest.(check bool) "no timeout ref" true (w.wb_timeout = None);
          Alcotest.(check bool) "no retry ref" true (w.wb_retry = None);
          Alcotest.(check bool)
            "two declared headers: a field ref and a template" true
            (match w.wb_request_headers with
            | [
             (_, Ir.Wire_field [ "api_key" ]);
             ( _,
               Ir.Wire_template [ Ir.Tpl_lit "v-"; Ir.Tpl_field [ "api_key" ] ]
             );
            ] ->
                true
            | _ -> false)
      | _ -> Alcotest.fail "nested op has no typed wire binding")
  | _ -> Alcotest.fail "client is not an entry"

(* ── Typecheck rejections ──────────────────────────────────────────────── *)

let wire_struct = "struct r { y: string }\n"

(* An entry wraps the given fields around one minimal valid op. *)
let entry fields =
  "pub struct c {\n" ^ fields
  ^ "\n\
    \  ep: string @env(\"EP\")\n\
    \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
     }\n" ^ wire_struct

let cycle_rejected () =
  Alcotest.(check (list string))
    "cycle" [ "TC0039" ]
    (codes
       (entry "  a: string @format(\"{.b}\")\n  b: string @format(\"x{.a}\")"))

let non_exhaustive_match_rejected () =
  Alcotest.(check (list string))
    "non-exhaustive" [ "TC0041" ]
    (codes
       (entry
          "  v: string @env(\"V\")\n  e: string = match .v { \"a\" => \"x\" }"))

let source_on_wire_struct_rejected () =
  (* A struct with a source is a config; crossing the wire as an op input and
     output is the closed boundary. *)
  Alcotest.(check (list string))
    "config on the wire" [ "TC0034"; "TC0034" ]
    (codes "struct s { k: string @env(\"K\") }\nop o(s): s")

let entry_as_op_io_rejected () =
  Alcotest.(check (list string))
    "entry as op input" [ "TC0034" ]
    (codes (entry "" ^ "op outer(c): r"))

let arg_excludes_other_sources () =
  Alcotest.(check (list string))
    "dead sources" [ "TC0036" ]
    (codes (entry "  k: string @arg @env(\"K\")"))

let match_with_sources_dead () =
  Alcotest.(check (list string))
    "match + sources" [ "TC0036" ]
    (codes
       (entry
          "  v: string @env(\"V\")\n\
          \  e: string = match .v { _ => \"x\" } @env(\"E\")"))

let bind_outside_composition_rejected () =
  Alcotest.(check (list string))
    "bind on a non-config field" [ "TC0042" ]
    (codes (entry "  k: string @env(\"K\") @bind(x, .ep)"))

let bind_unknown_target_rejected () =
  Alcotest.(check (list string))
    "bind target not in config" [ "TC0042" ]
    (codes
       ("struct conf { a: string @env(\"A\") }\n"
       ^ entry "  s: conf @bind(nope, .ep)"))

let lazy_chain_named_at_consumption () =
  let diags =
    check
      ("pub struct c {\n\
       \  endpoint_v2: string @env(.env_name)\n\
       \  env_name: string\n\
       \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .endpoint_v2)\n\
        }\n" ^ wire_struct)
  in
  Alcotest.(check (list string))
    "one lazy error" [ "TC0037" ]
    (List.filter_map (fun (d : Diagnostic.t) -> d.code) diags);
  let msg = (List.hd diags).message in
  Alcotest.(check bool)
    "chain is described" true
    (contains msg "endpoint_v2 <- env_name")

let unconsumed_field_without_source_rejected () =
  Alcotest.(check (list string))
    "no source" [ "TC0037" ]
    (codes (entry "  orphan: string"))

let unknown_ref_rejected () =
  Alcotest.(check (list string))
    "unknown env ref" [ "TC0038" ]
    (codes (entry "  k: string @env(.nope)"))

let op_ref_unknown_rejected () =
  Alcotest.(check (list string))
    "unknown op trait ref" [ "TC0038" ]
    (codes
       ("pub struct c {\n\
        \  ep: string @env(\"EP\")\n\
        \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
         @header(\"X\", .nope)\n\
         }\n" ^ wire_struct))

let endpoint_required_on_entry_http_op () =
  Alcotest.(check (list string))
    "endpoint missing" [ "TC0043" ]
    (codes
       ("pub struct c {\n\
        \  ep: string @env(\"EP\")\n\
        \  op o(): r @http(method: \"GET\", path: \"/\")\n\
         }\n" ^ wire_struct))

let protocol_trait_requires_http () =
  Alcotest.(check (list string))
    "timeout without http" [ "TC0044" ]
    (codes
       ("pub struct c {\n\
        \  ep: string @env(\"EP\")\n\
        \  t: duration @with\n\
        \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
        \  op p(): r @timeout(.t)\n\
         }\n\
         ext impl p { go: \"ext/go/p.go#P\" }\n" ^ wire_struct))

let loose_op_ref_rejected () =
  Alcotest.(check (list string))
    "field ref on a loose op" [ "TC0044" ]
    (codes
       "struct w { x: string }\n\
        op o(w): w @http(method: \"GET\", path: \"/\", endpoint: .x)")

let unknown_transform_rejected () =
  Alcotest.(check (list string))
    "unknown transform" [ "TC0045" ]
    (codes (entry "  k: string @env(\"K\") @str::shout"))

let nullable_entry_field_rejected () =
  Alcotest.(check (list string))
    "nullable field" [ "TC0046" ]
    (codes (entry "  k: string? @env(\"K\")"))

let config_with_arg_rejected () =
  Alcotest.(check (list string))
    "arg inside config" [ "TC0035" ]
    (codes
       ("struct conf { a: string @arg, b: string @env(\"B\") }\n"
       ^ entry "  s: conf @bind(a, .ep)"))

let () =
  Alcotest.run "entries"
    [
      ( "accept",
        [
          Alcotest.test_case "canonical forms parse and check" `Quick
            canonical_accepted;
          Alcotest.test_case "loose ops untouched" `Quick loose_ops_untouched;
        ] );
      ( "lower",
        [
          Alcotest.test_case "entry surface" `Quick lowered_entry;
          Alcotest.test_case "config surface" `Quick lowered_config;
          Alcotest.test_case "bind-only config" `Quick bind_only_config;
        ] );
      ( "ir",
        [
          Alcotest.test_case "round-trip" `Quick ir_roundtrip;
          Alcotest.test_case "version 9" `Quick version_is_9;
        ] );
      ("fmt", [ Alcotest.test_case "round-trip" `Quick fmt_roundtrip ]);
      ( "protocol",
        [
          Alcotest.test_case "descriptor refs" `Quick descriptor_carries_refs;
          Alcotest.test_case "loose descriptor" `Quick
            loose_descriptor_has_no_entry_fields;
          Alcotest.test_case "wire field: query/payload/response" `Quick
            wire_field_covers_query_payload_and_response_bindings;
        ] );
      ( "reject",
        [
          Alcotest.test_case "cycle" `Quick cycle_rejected;
          Alcotest.test_case "non-exhaustive match" `Quick
            non_exhaustive_match_rejected;
          Alcotest.test_case "source on wire struct" `Quick
            source_on_wire_struct_rejected;
          Alcotest.test_case "entry as op io" `Quick entry_as_op_io_rejected;
          Alcotest.test_case "arg excludes sources" `Quick
            arg_excludes_other_sources;
          Alcotest.test_case "match with sources" `Quick match_with_sources_dead;
          Alcotest.test_case "bind outside composition" `Quick
            bind_outside_composition_rejected;
          Alcotest.test_case "bind unknown target" `Quick
            bind_unknown_target_rejected;
          Alcotest.test_case "lazy chain" `Quick lazy_chain_named_at_consumption;
          Alcotest.test_case "field without source" `Quick
            unconsumed_field_without_source_rejected;
          Alcotest.test_case "unknown ref" `Quick unknown_ref_rejected;
          Alcotest.test_case "unknown op ref" `Quick op_ref_unknown_rejected;
          Alcotest.test_case "endpoint required" `Quick
            endpoint_required_on_entry_http_op;
          Alcotest.test_case "protocol trait needs http" `Quick
            protocol_trait_requires_http;
          Alcotest.test_case "loose op ref" `Quick loose_op_ref_rejected;
          Alcotest.test_case "unknown transform" `Quick
            unknown_transform_rejected;
          Alcotest.test_case "nullable field" `Quick
            nullable_entry_field_rejected;
          Alcotest.test_case "arg in config" `Quick config_with_arg_rejected;
        ] );
    ]
