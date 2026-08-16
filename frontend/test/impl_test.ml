open Tono_frontend

(* The implementation count and the raw-form rules (TC0048-TC0053): an "ext impl"
   names an operation an entry declares, exactly one thing implements an
   operation, and a raw implementation can only select a declared error that
   carries an @errorCode. *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

(* A hybrid entry: one operation reaches a protocol, one is implemented by
   bespoke sources in the typed form, one in the raw form. *)
let impl_entry body =
  "pub struct client {\n\
  \  ep: string @env(\"EP\")\n\
  \  op fetch(): note @http(method: \"GET\", path: \"/n\", endpoint: .ep)\n\
  \  op save(note): note\n\
   }\n\
   struct note { id: string }\n" ^ body

let impl_clean () =
  Alcotest.(check (list string))
    "no codes" []
    (codes (impl_entry "ext impl save raw { go: \"ext/go/s.go#Save\" }"))

let impl_qualified_name_clean () =
  Alcotest.(check (list string))
    "entry.op resolves" []
    (codes (impl_entry "ext impl client.save { go: \"ext/go/s.go#Save\" }"))

let impl_unknown_op () =
  Alcotest.(check (list string))
    "impl names no operation" [ "TC0048" ]
    (codes
       (impl_entry
          "ext impl save { go: \"ext/go/s.go#Save\" }\n\
           ext impl purge { go: \"ext/go/p.go#Purge\" }"))

let impl_ambiguous_op () =
  Alcotest.(check (list string))
    "bare name reaches two entries" [ "TC0049" ]
    (codes
       "pub struct a {\n\
       \  ep: string @env(\"EP\")\n\
       \  op save(): note @http(method: \"GET\", path: \"/n\", endpoint: .ep)\n\
        }\n\
        pub struct b {\n\
       \  ep: string @env(\"EP\")\n\
       \  op save(): note\n\
        }\n\
        struct note { id: string }\n\
        ext impl save { go: \"ext/go/s.go#Save\" }\n\
        ext impl b.save { go: \"ext/go/s.go#SaveB\" }")

let impl_conflicts_with_http () =
  Alcotest.(check (list string))
    "@http plus impl" [ "TC0050" ]
    (codes
       (impl_entry
          "ext impl fetch { go: \"ext/go/f.go#Fetch\" }\n\
           ext impl save { go: \"ext/go/s.go#Save\" }"))

let impl_duplicate_for_one_op () =
  Alcotest.(check (list string))
    "two impls reach one operation" [ "TC0050" ]
    (codes
       (impl_entry
          "ext impl save { go: \"ext/go/s.go#Save\" }\n\
           ext impl client.save { ts: \"ext/ts/s.ts#save\" }"))

let impl_missing () =
  Alcotest.(check (list string))
    "entry op with no implementation" [ "TC0051" ]
    (codes (impl_entry ""))

(* A loose operation is a bare contract, so it needs no implementation. *)
let loose_op_needs_no_impl () =
  Alcotest.(check (list string))
    "loose op stays clean" []
    (codes "struct note { id: string }\nop save(note): note")

(* ...and cannot take one either: no client exposes it, so the binding would be
   declared and then never called. *)
let impl_on_loose_op () =
  Alcotest.(check (list string))
    "impl on a loose op" [ "TC0048" ]
    (codes
       "struct note { id: string }\n\
        op save(note): note\n\
        ext impl save { go: \"ext/go/s.go#Save\" }")

let impl_with_signature () =
  Alcotest.(check (list string))
    "impl declares no signature" [ "TC0029" ]
    (codes
       (impl_entry "ext impl save (note) -> note { go: \"ext/go/s.go#Save\" }"))

(* A raw implementation matches a failure on its code alone, so a declared error
   without one can never be selected. *)
let raw_error_without_code_warns () =
  Alcotest.(check (list string))
    "unreachable declared error" [ "TC0053" ]
    (codes
       "pub struct client {\n\
       \  ep: string @env(\"EP\")\n\
       \  op save(note): note @errors(busy)\n\
        }\n\
        struct note { id: string }\n\
        @status(529) struct busy { message: string }\n\
        ext impl save raw { go: \"ext/go/s.go#Save\" }")

let raw_error_with_code_clean () =
  Alcotest.(check (list string))
    "reachable declared error" []
    (codes
       "pub struct client {\n\
       \  ep: string @env(\"EP\")\n\
       \  op save(note): note @errors(busy)\n\
        }\n\
        struct note { id: string }\n\
        @status(529) @errorCode(\"code\", \"busy\") struct busy { message: \
        string }\n\
        ext impl save raw { go: \"ext/go/s.go#Save\" }")

(* The typed form returns declared errors as typed values, so it never
   discriminates and the code is not required. *)
let typed_impl_needs_no_error_code () =
  Alcotest.(check (list string))
    "typed impl stays clean" []
    (codes
       "pub struct client {\n\
       \  ep: string @env(\"EP\")\n\
       \  op save(note): note @errors(busy)\n\
        }\n\
        struct note { id: string }\n\
        @status(529) struct busy { message: string }\n\
        ext impl save { go: \"ext/go/s.go#Save\" }")

let raw_outside_impl () =
  Alcotest.(check (list string))
    "raw on a constraint" [ "TC0052" ]
    (codes
       (impl_entry
          "ext impl save { go: \"ext/go/s.go#Save\" }\n\
           ext constraint luhn raw (string) -> bool { ts: \"ext/ts/l.ts#f\" }"))

(* An op's own "impl .field.method(args)" body (RFC-0023) and an "ext impl"
   are two more of the three implementation sources; binding both to the same
   op is the same conflict as @http plus an impl. *)
let op_impl_conflicts_with_ext_impl () =
  Alcotest.(check (list string))
    "op impl and ext impl both bind the same op" [ "TC0050" ]
    (codes
       "ext bus {\n\
       \  go: \"github.com/x/bus\"\n\
       \  struct go_ack { OK: bool }\n\
       \  type publisher {\n\
       \    extern send(topic: string): ack {\n\
       \      go { call: \"Send\"(topic) yields: (a: go_ack) returns: ack { \
        accepted: .a.OK } }\n\
       \    }\n\
       \  }\n\
       \  extern connect(endpoint: string): publisher {\n\
       \    go { call: \"Connect\"(endpoint) }\n\
       \  }\n\
        }\n\
        pub struct ack { accepted: bool }\n\
        pub struct client {\n\
       \  endpoint: string @arg\n\
       \  bus: bus.publisher @with = bus.connect(.endpoint)\n\
       \  op publish(topic: string): ack\n\
       \    impl .bus.send(.topic)\n\
        }\n\
        ext impl publish { go: \"ext/go/p.go#Publish\" }")

let () =
  Alcotest.run "impl"
    [
      ( "impl",
        [
          Alcotest.test_case "clean" `Quick impl_clean;
          Alcotest.test_case "qualified name" `Quick impl_qualified_name_clean;
          Alcotest.test_case "unknown op" `Quick impl_unknown_op;
          Alcotest.test_case "ambiguous op" `Quick impl_ambiguous_op;
          Alcotest.test_case "conflicts with @http" `Quick
            impl_conflicts_with_http;
          Alcotest.test_case "duplicate for one op" `Quick
            impl_duplicate_for_one_op;
          Alcotest.test_case "missing implementation" `Quick impl_missing;
          Alcotest.test_case "loose op needs none" `Quick loose_op_needs_no_impl;
          Alcotest.test_case "impl on a loose op" `Quick impl_on_loose_op;
          Alcotest.test_case "impl with signature" `Quick impl_with_signature;
          Alcotest.test_case "raw outside impl" `Quick raw_outside_impl;
          Alcotest.test_case "raw error without code" `Quick
            raw_error_without_code_warns;
          Alcotest.test_case "raw error with code" `Quick
            raw_error_with_code_clean;
          Alcotest.test_case "typed impl needs no code" `Quick
            typed_impl_needs_no_error_code;
          Alcotest.test_case "op impl conflicts with ext impl" `Quick
            op_impl_conflicts_with_ext_impl;
        ] );
    ]
