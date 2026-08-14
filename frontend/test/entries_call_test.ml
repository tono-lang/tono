open Tono_frontend

(* The extern-call field source (RFC-0023): source-combination rules, DAG
   cycle detection over call-arg refs, unresolved-ref reporting, the
   `.request` rejection (decision M), and a two-segment projection into a
   call result. Split out of [entries_edge_test.ml] to stay under this repo's
   per-file line ceiling. *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

let expect what wanted src =
  Alcotest.(check (list string)) what wanted (codes src)

let wire = "struct r { y: string }\n"

let entry fields =
  "pub struct c {\n" ^ fields
  ^ "\n\
    \  ep: string @env(\"EP\")\n\
    \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
     }\n" ^ wire

let call_and_arg_rejected () =
  expect "call plus @arg" [ "TC0036" ]
    (entry "  config: string = ns.load() @arg")

let call_and_env_rejected () =
  expect "call plus @env" [ "TC0036" ]
    (entry "  config: string = ns.load() @env(\"C\")")

let call_with_with_accepted () =
  expect "call with an @with fallback slot" []
    (entry "  config: string = ns.load() @with")

let call_cycle_rejected () =
  expect "cycle across call refs" [ "TC0039" ]
    (entry "  a: string = ns.f(.b)\n  b: string = ns.g(.a)")

let call_arg_unresolved_ref_rejected () =
  expect "unresolved call arg ref" [ "TC0038" ]
    (entry "  config: string = ns.load(.nope)")

let call_arg_request_ref_rejected () =
  expect "request ref in call arg" [ "TC0085" ]
    (entry "  config: string = ns.load(.request)")

let call_two_segment_projection_accepted () =
  expect "two-segment projection into a call result" []
    ("struct app_config { token: string }\n"
    ^ entry
        "  config: app_config = ns.load()\n\
        \  auth: string @format(\"Bearer {.config.token}\")")

let config_member_call_request_ref_rejected () =
  expect "request ref in a config member's call" [ "TC0085" ]
    ("struct conf {\n  a: string = ns.load(.request)\n}\n"
    ^ entry "  s: conf @bind(a, .ep)")

let config_member_call_unresolved_ref_rejected () =
  expect "unresolved ref in a config member's call" [ "TC0038" ]
    ("struct conf {\n  a: string = ns.load(.nope)\n}\n"
    ^ entry "  s: conf @bind(a, .ep)")

(* Every [call_arg]/[trait_arg] shape the surface can produce inside a field
   call: a bare parameter, a literal, a plain ref, and a ctor argument whose
   own fields are, in turn, a ref, a list, a nested ctor, a nested call, and a
   literal (the ctor-field catch-all). Exercises every branch of
   [Entry_scope.ce_refs]/[trait_arg_refs] in one field, so none of them stays
   silently untested as the grammar grows. *)
let call_arg_rich_forms_all_resolve () =
  expect "every call-arg/trait-arg shape resolves" []
    (entry
       "  a: string @arg\n\
       \  b: string @arg\n\
       \  c: string @arg\n\
       \  d: string @arg\n\
       \  config: string = ns.load(a, 3, .a, sub {\n\
       \    ref_field: .b,\n\
       \    list_field: [.a, .b],\n\
       \    nested: inner { x: .c },\n\
       \    hook: ns2.other(.d),\n\
       \    label: \"lit\"\n\
       \  })")

let () =
  Alcotest.run "entries_call"
    [
      ( "extern-call",
        [
          Alcotest.test_case "call plus @arg" `Quick call_and_arg_rejected;
          Alcotest.test_case "call plus @env" `Quick call_and_env_rejected;
          Alcotest.test_case "call with @with fallback" `Quick
            call_with_with_accepted;
          Alcotest.test_case "cycle across call refs" `Quick call_cycle_rejected;
          Alcotest.test_case "unresolved call arg ref" `Quick
            call_arg_unresolved_ref_rejected;
          Alcotest.test_case "request ref in call arg" `Quick
            call_arg_request_ref_rejected;
          Alcotest.test_case "two-segment projection" `Quick
            call_two_segment_projection_accepted;
          Alcotest.test_case "config member request ref" `Quick
            config_member_call_request_ref_rejected;
          Alcotest.test_case "config member unresolved ref" `Quick
            config_member_call_unresolved_ref_rejected;
          Alcotest.test_case "rich call-arg forms" `Quick
            call_arg_rich_forms_all_resolve;
        ] );
    ]
