open Tono_frontend

(* Positions where entry-model constructions used to be accepted and then
   dropped in silence: wire members carrying selection/derivation metadata,
   loose ops with entry-only protocol traits, templates inside @env names,
   non-string header values, and duplicated non-repeatable traits (the
   trailing-trait absorption footgun now yields a diagnostic). *)

let check src =
  let file, _ = Parser.parse src in
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

let expect what wanted src =
  Alcotest.(check (list string)) what wanted (codes src)

let wire = "struct r { y: string }\n"

let entry fields =
  "pub struct c {\n" ^ fields
  ^ "\n\
    \  ep: string @env(\"EP\")\n\
    \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
     }\n" ^ wire

(* ── Silent-drop positions get diagnostics ─────────────────────────────── *)

let wire_member_match_rejected () =
  expect "match on a wire member" [ "TC0035" ]
    "struct s { v: string, e: string = match .v { _ => \"x\" } }"

let wire_member_format_rejected () =
  expect "format on a wire member" [ "TC0035" ]
    "struct s { a: string @format(\"{.b}\"), b: string }"

let wire_member_transform_rejected () =
  expect "transform on a wire member" [ "TC0035" ]
    "struct s { a: string @str::trim }"

let wire_member_bind_rejected () =
  expect "bind on a wire member" [ "TC0042" ]
    "struct s { a: string @bind(x, .a) }"

let loose_op_literal_timeout_rejected () =
  expect "literal timeout on a loose op" [ "TC0044" ]
    "struct w { x: string }\n\
     op o(w): w @http(method: \"GET\", path: \"/\") @timeout(5)"

let loose_op_literal_retry_rejected () =
  expect "literal retry on a loose op" [ "TC0044" ]
    "struct w { x: string }\n\
     op o(w): w @http(method: \"GET\", path: \"/\") @retry(3)"

let env_with_placeholder_rejected () =
  expect "template inside @env" [ "TC0035" ]
    (entry "  k: string @env(\"ENDPOINT_{.ep}\")")

let header_literal_int_value_rejected () =
  expect "int header value" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"K\", 5)\n\
      }\n" ^ wire)

let absorbed_doc_duplicate_rejected () =
  (* The known footgun: a @doc on its own line between an op and the next
     field binds to the op; with a @doc already there, the duplicate now
     yields a diagnostic instead of silently doubling. *)
  expect "absorbed doc duplicates" [ "TC0047" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @doc(\"api\")\n\
     \  @doc(\"meant for the field below\")\n\
     \  k: string @env(\"K\")\n\
      }\n" ^ wire)

let duplicate_doc_on_decl_rejected () =
  expect "duplicate decl doc" [ "TC0047" ]
    "@doc(\"a\")\n@doc(\"b\")\nstruct s { x: string }"

let repeatable_traits_stay_legal () =
  expect "repeated env and errors stay legal" []
    ("@status(500) @errorCode(\"a\") struct e1 { m: string }\n\
      @status(501) @errorCode(\"b\") struct e2 { m: string }\n"
    ^ entry
        "  k: string @env(\"A\") @env(\"B\") @default(\"x\")\n\
        \  op p(): r @http(method: \"GET\", path: \"/p\", endpoint: .ep) \
         @errors(e1, e2) @header(\"A\", .k) @header(\"B\", .k)")

let config_boundary_names_composition_point () =
  let diags =
    check
      ("struct conf { api_key: string }\n"
      ^ entry "  settings: conf @bind(api_key, .ep)"
      ^ "op outer(conf): r")
  in
  Alcotest.(check (list string))
    "one boundary error" [ "TC0034" ]
    (List.filter_map (fun (d : Diagnostic.t) -> d.code) diags);
  Alcotest.(check bool)
    "message cites the composition point" true
    (contains (List.hd diags).message
       "composes it via @bind on field 'settings'")

let () =
  Alcotest.run "entries_position"
    [
      ( "silent-drop",
        [
          Alcotest.test_case "wire member match" `Quick
            wire_member_match_rejected;
          Alcotest.test_case "wire member format" `Quick
            wire_member_format_rejected;
          Alcotest.test_case "wire member transform" `Quick
            wire_member_transform_rejected;
          Alcotest.test_case "wire member bind" `Quick wire_member_bind_rejected;
          Alcotest.test_case "loose literal timeout" `Quick
            loose_op_literal_timeout_rejected;
          Alcotest.test_case "loose literal retry" `Quick
            loose_op_literal_retry_rejected;
          Alcotest.test_case "env with placeholder" `Quick
            env_with_placeholder_rejected;
          Alcotest.test_case "int header value" `Quick
            header_literal_int_value_rejected;
          Alcotest.test_case "absorbed doc duplicate" `Quick
            absorbed_doc_duplicate_rejected;
          Alcotest.test_case "duplicate decl doc" `Quick
            duplicate_doc_on_decl_rejected;
          Alcotest.test_case "repeatable traits legal" `Quick
            repeatable_traits_stay_legal;
          Alcotest.test_case "config boundary hint" `Quick
            config_boundary_names_composition_point;
        ] );
    ]
