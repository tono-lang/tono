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

let version_is_7 () =
  Alcotest.(check int) "wire version" 7 Ir_json.current_ir_version

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

(* ── Protocol: entry refs land in the wire descriptor ──────────────────── *)

let descriptor_carries_refs () =
  let m = Protocol_http.resolve_module (compile canonical_client) in
  let client = shape_by_id m "m#client" in
  match client.kind with
  | Ir.Entry { operations; _ } ->
      let op = List.hd operations in
      let desc =
        match
          List.find_opt
            (fun (t : Ir.trait) -> t.trait_id = "wire_descriptor")
            op.traits
        with
        | Some t -> t.value
        | None -> Alcotest.fail "nested op has no wire_descriptor"
      in
      let get k =
        match desc with `Assoc kvs -> List.assoc_opt k kvs | _ -> None
      in
      Alcotest.(check bool)
        "endpoint ref" true
        (get "endpoint" = Some (`List [ `String "endpoint" ]));
      Alcotest.(check bool)
        "timeout is a value-source ref" true
        (get "timeout" = Some (`Assoc [ ("ref", `String "timeout") ]));
      Alcotest.(check bool)
        "retry wraps its max value-source" true
        (get "retry"
        = Some (`Assoc [ ("max", `Assoc [ ("ref", `String "max_retries") ]) ]));
      (* A declared error's (status, @errorCode, @retryable) is the generated
         SDK's own decode<Op>Error + .retryable() pair to own; the descriptor
         carries no parallel copy. *)
      Alcotest.(check bool) "no errors key" true (get "errors" = None);
      Alcotest.(check bool)
        "one declared header" true
        (match get "request_headers" with
        | Some
            (`List [ `List [ _key; `Assoc [ ("field", `List [ `String f ]) ] ] ])
          ->
            String.equal f "client_name"
        | _ -> false);
      Alcotest.(check bool)
        "uri keeps both placeholder scopes verbatim" true
        (get "uri" = Some (`String "/notes/{id}"))
  | _ -> Alcotest.fail "client is not an entry"

let loose_descriptor_has_no_entry_fields () =
  let m =
    Protocol_http.resolve_module
      (compile
         "struct w { x: string }\n\
          op o(w): w @http(method: \"GET\", path: \"/w\")")
  in
  let op = List.hd m.operations in
  let desc =
    match
      List.find_opt
        (fun (t : Ir.trait) -> t.trait_id = "wire_descriptor")
        op.traits
    with
    | Some t -> t.value
    | None -> Alcotest.fail "loose op has no wire_descriptor"
  in
  match desc with
  | `Assoc kvs ->
      Alcotest.(check bool)
        "no entry-scoped keys" true
        (List.for_all
           (fun k -> not (List.mem_assoc k kvs))
           [ "endpoint"; "request_headers"; "timeout"; "retry" ])
  | _ -> Alcotest.fail "descriptor is not an object"

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
          Alcotest.test_case "version 7" `Quick version_is_7;
        ] );
      ("fmt", [ Alcotest.test_case "round-trip" `Quick fmt_roundtrip ]);
      ( "protocol",
        [
          Alcotest.test_case "descriptor refs" `Quick descriptor_carries_refs;
          Alcotest.test_case "loose descriptor" `Quick
            loose_descriptor_has_no_entry_fields;
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
