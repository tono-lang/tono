open Tono_frontend

(* ── Helpers ───────────────────────────────────────────────────────────── *)

let errors_of ds =
  List.filter (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) ds

let contains hay needle =
  let hn = String.length hay and nn = String.length needle in
  let rec loop i =
    if i + nn > hn then false
    else if String.sub hay i nn = needle then true
    else loop (i + 1)
  in
  nn = 0 || loop 0

(* Parse + lower + typecheck one module, returning the module and every
   diagnostic (the declared-test pass runs inside [Typecheck.check_module]). *)
let compile src = Tono_frontend.compile ~module_name:"github" src

let codes src =
  List.filter_map (fun (d : Diagnostic.t) -> d.code) (snd (compile src))

(* A GitHub-like base: an @http op, a bespoke impl op with a declared error,
   and a no-output op, so tests can exercise every dependency and pattern. *)
let base =
  {|
import tono.http
import tono.errors

pub struct user {
  login: string
  id: i32
  name: string?
  bio: string?
}

pub struct user_ref {
  username: string
}

pub struct note {
  id: string
  body: string
}

@status(529) @errorCode("code", "overloaded")
pub struct overloaded {
  message: string
}

pub struct client {
  api_token: string @arg
  endpoint: string @env("GITHUB_ENDPOINT") @default("https://api.github.com")

  op get_user(user_ref: user_ref): user
    @http(method: "GET", path: "/users/{.user_ref.username}", endpoint: .endpoint)

  op save_note(note: note): note @errors(overloaded)

  op ping()
    @http(method: "GET", path: "/ping", endpoint: .endpoint)
}

ext impl save_note {
  go: "ext/go/notes.go#SaveNote"
}
|}

let with_base body = base ^ "\n" ^ body

(* ── The happy path ────────────────────────────────────────────────────── *)

(* Every RFC form at once: construction, named http stub with a sequence,
   anonymous stub, impl stub answers (output, declared error, contract), calls
   with and without input, dataflow, struct/error/taxonomy/ok patterns with
   the three marks, and request assertions with a header subset. *)
let rich_src =
  with_base
    {|
test "hermetic fixture decodes as declared" {
  c: client { api_token: "t0" }
  s: stub c.get_user.http: [
    http.response { status: 200, headers: { "content-type": "application/json" }, body: "{}" },
    http.response { status: 503, body: "busy" }
  ]
  got: c.get_user(user_ref { username: "octocat" })
  expect got: user { login: "octocat", id: 583231, bio: None, name: any, .. }
  expect s.requests: [http.request { method: "GET", path: "/users/octocat", headers: { "accept": "application/json", .. }, .. }]
}

test "construction rejects an empty token" {
  c: client { api_token: "" }
  expect c: errors.contract { name: "client_init" }
}

test "the glue guards the store's declared failure" {
  c: client { api_token: "t0" }
  stub c.save_note.impl: [
    note { id: "n1", body: "hello" },
    overloaded { message: "simulated shedding" },
    errors.contract {}
  ]
  saved: c.save_note(note { id: "n1", body: "hello" })
  expect saved: overloaded { message: "simulated shedding" }
}

test "flows chain with dataflow" {
  c: client { api_token: "t0" }
  saved: c.save_note(note { id: "n7", body: "hello" })
  expect saved: note { id: "n7", .. }
  fetched: c.save_note(note { id: saved.id, body: "again" })
  expect fetched: note { id: saved.id, body: "again" }
}

test "an op without output asserts ok" {
  c: client {}
  done: c.ping()
  expect done: ok
  expect c: ok
}

test "fallbacks surface as the taxonomy" {
  c: client {}
  stub c.get_user.http: http.response { status: 404, body: "{\"message\":\"Not Found\"}" }
  got: c.get_user(user_ref { username: "nobody" })
  expect got: errors.api { status: 404, .. }
}
|}

let rich_compiles_clean () =
  let m, ds = compile rich_src in
  Alcotest.(check (list string))
    "no diagnostics" []
    (List.map Diagnostic.to_string ds);
  Alcotest.(check int) "six tests" 6 (List.length m.Ir.tests)

(* The lowered IR carries the spec encoding: wire-form values, grouped
   sections, stub answers per dependency, and the pattern marks. *)
let lowered_shape () =
  let m, _ = compile rich_src in
  let t = List.hd m.Ir.tests in
  Alcotest.(check string)
    "name" "hermetic fixture decodes as declared" t.Ir.t_name;
  (match t.Ir.t_constructions with
  | [ c ] ->
      Alcotest.(check string) "entry" "client" c.Ir.tc_entry;
      Alcotest.(check bool)
        "pinned value" true
        (c.Ir.tc_values = [ ("api_token", `String "t0") ])
  | _ -> Alcotest.fail "one construction expected");
  (match t.Ir.t_stubs with
  | [ s ] ->
      Alcotest.(check bool) "named" true (s.Ir.ts_binding = Some "s");
      Alcotest.(check bool) "http dep" true (s.Ir.ts_dep = Ir.Dep_http);
      Alcotest.(check int) "sequence" 2 (List.length s.Ir.ts_answers)
  | _ -> Alcotest.fail "one stub expected");
  match t.Ir.t_expects with
  | [
   Ir.Expect_outcome { ex_pattern = Ir.P_struct p; _ };
   Ir.Expect_requests { ex_requests = [ rp ]; _ };
  ] ->
      Alcotest.(check string) "shape" "user" p.ps_shape;
      Alcotest.(check bool) "open" true p.ps_open;
      Alcotest.(check bool)
        "absent mark" true
        (List.assoc "bio" p.ps_fields = Ir.Fp_absent);
      Alcotest.(check bool)
        "present mark" true
        (List.assoc "name" p.ps_fields = Ir.Fp_present);
      Alcotest.(check bool) "request open" true rp.Ir.rp_open;
      Alcotest.(check bool) "headers subset" true (rp.Ir.rp_headers <> None)
  | _ -> Alcotest.fail "outcome + requests expects expected"

(* Impl answers lower to the three forms; dataflow to a $ref leaf. *)
let impl_and_dataflow () =
  let m, _ = compile rich_src in
  let impl_test = List.nth m.Ir.tests 2 in
  (match impl_test.Ir.t_stubs with
  | [ { Ir.ts_dep = Ir.Dep_impl; ts_answers = [ a; b; c ]; _ } ] ->
      (match a with
      | Ir.Answer_value (`Assoc kvs) ->
          Alcotest.(check bool)
            "typed output" true
            (List.assoc "id" kvs = `String "n1")
      | _ -> Alcotest.fail "value answer expected");
      (match b with
      | Ir.Answer_error { ans_shape; _ } ->
          Alcotest.(check string) "error shape" "overloaded" ans_shape
      | _ -> Alcotest.fail "error answer expected");
      Alcotest.(check bool) "contract answer" true (c = Ir.Answer_contract)
  | _ -> Alcotest.fail "impl stub expected");
  let flow = List.nth m.Ir.tests 3 in
  match flow.Ir.t_calls with
  | [ _; { Ir.call_input = Some (`Assoc kvs); _ } ] ->
      Alcotest.(check bool)
        "$ref leaf" true
        (List.assoc "id" kvs
        = `Assoc
            [
              ( "$ref",
                `Assoc
                  [
                    ("binding", `String "saved");
                    ("path", `List [ `String "id" ]);
                  ] );
            ])
  | _ -> Alcotest.fail "two calls expected"

(* i64 pins the wire form: integers travel as strings. *)
let wire_i64 () =
  let src =
    {|
pub struct row { n: i64 }
pub struct row_ref { id: string }
pub struct client {
  endpoint: string @env("EP") @default("https://x")
  op get(row_ref: row_ref): row
    @http(method: "GET", path: "/rows/{id}", endpoint: .endpoint)
}
test "t" {
  c: client {}
  got: c.get(row_ref { id: "r1" })
  expect got: row { n: 583231 }
}
|}
  in
  let m, ds = compile src in
  Alcotest.(check (list string)) "clean" [] (List.map Diagnostic.to_string ds);
  match (List.hd m.Ir.tests).Ir.t_expects with
  | [ Ir.Expect_outcome { ex_pattern = Ir.P_struct p; _ } ] ->
      Alcotest.(check bool)
        "i64 as string" true
        (List.assoc "n" p.ps_fields = Ir.Fp_pat (Ir.P_eq (`String "583231")))
  | _ -> Alcotest.fail "one expect expected"

(* ── IR round-trip ─────────────────────────────────────────────────────── *)

let canon j = Ir_json.to_canonical_string j

let roundtrip () =
  let m, ds = compile rich_src in
  Alcotest.(check int) "clean" 0 (List.length (errors_of ds));
  let json = Ir_json.encode_module m in
  match Ir_json.decode_module json with
  | Error e -> Alcotest.failf "module did not round-trip: %s" e
  | Ok m' ->
      Alcotest.(check string)
        "re-encode is identical" (canon json)
        (canon (Ir_json.encode_module m'))

(* The tests list is always written, so an empty module round-trips its
   (empty) table byte for byte, like extensions. *)
let tests_key_always_written () =
  let m, _ = compile "pub struct a { x: string }" in
  match Ir_json.encode_module m with
  | `Assoc kvs ->
      Alcotest.(check bool)
        "tests key present" true
        (List.assoc_opt "tests" kvs = Some (`List []))
  | _ -> Alcotest.fail "module must encode to an object"

(* ── Parser rejections ─────────────────────────────────────────────────── *)

let parse_errors src =
  let _, ds = Parser.parse src in
  List.length (errors_of ds)

let parser_rejects () =
  let cases =
    [
      ("missing name", "test { }");
      ("pub test", "pub test \"t\" {}");
      ("trait on test", "@doc(\"x\") test \"t\" {}");
      ("dots in value ctor", "test \"t\" { c: client { .. } }");
      ("pattern mark in value", "test \"t\" { c: client { a: any } }");
      ("bad projection", "test \"t\" { expect s.foo: ok }");
      ("stub path too short", "test \"t\" { stub c: ok }");
    ]
  in
  List.iter
    (fun (name, src) -> Alcotest.(check bool) name true (parse_errors src > 0))
    cases

(* Outside a test block the contextual words stay ordinary identifiers. *)
let contextual_words_stay_identifiers () =
  let src =
    "pub struct stub { expect: string, any: string }\n\
     op expect(stub: stub): stub"
  in
  let _, ds = Parser.parse src in
  Alcotest.(check int) "no parse errors" 0 (List.length (errors_of ds))

(* ── Typecheck diagnostics, one per code ───────────────────────────────── *)

let tc name expected body =
  Alcotest.test_case name `Quick (fun () ->
      Alcotest.(check (list string)) name expected (codes (with_base body)))

let binding_cases =
  [
    tc "unknown expect subject (TC0055)" [ "TC0055" ]
      {|test "t" { expect nope: ok }|};
    (* "c" is neither a construction binding yet (forward reference) nor a
       declared 'ext' library, so the stub's path is diagnosed as unresolved
       rather than the binding itself as unknown. *)
    tc "forward reference (TC0057)" [ "TC0057" ]
      {|test "t" { stub c.get_user.http: http.response { status: 200 } c: client {} expect c: ok }|};
    tc "call receiver not a construction (TC0055)" [ "TC0055" ]
      {|test "t" { c: client {} s: stub c.get_user.http: http.response { status: 200 } got: s.get_user(user_ref { username: "x" }) expect c: ok }|};
    tc "unknown op (TC0056)" [ "TC0056" ]
      {|test "t" { c: client {} got: c.nope(user_ref { username: "x" }) expect c: ok }|};
    tc "http dep without @http (TC0057)" [ "TC0057" ]
      {|test "t" { c: client {} stub c.save_note.impl: errors.contract {} stub c.save_note.http: http.response { status: 200 } expect c: ok }|};
    tc "impl dep without ext impl (TC0057)" [ "TC0057" ]
      {|test "t" { c: client {} stub c.get_user.impl: errors.contract {} expect c: ok }|};
    tc "unknown dep (TC0057)" [ "TC0057" ]
      {|test "t" { c: client {} stub c.get_user.tcp: http.response { status: 200 } expect c: ok }|};
    tc "http stub answered with a user shape (TC0058)" [ "TC0058" ]
      {|test "t" { c: client {} stub c.get_user.http: user { login: "x", id: 1 } expect c: ok }|};
    tc "impl stub answered off-language (TC0058)" [ "TC0058" ]
      {|test "t" { c: client {} stub c.save_note.impl: user { login: "x", id: 1 } expect c: ok }|};
    tc "empty stub sequence (TC0058)" [ "TC0058" ]
      {|test "t" { c: client {} stub c.get_user.http: [] expect c: ok }|};
    tc "construction field typo (TC0059)" [ "TC0059" ]
      {|test "t" { c: client { api_tokn: "x" } expect c: ok }|};
    tc "construction value type (TC0059)" [ "TC0059" ]
      {|test "t" { c: client { api_token: 1 } expect c: ok }|};
    (* The failed construction binds nothing, so the expect also misses (a
       deliberate cascade: the first error is the fix). *)
    tc "unknown entry (TC0059)" [ "TC0059"; "TC0055" ]
      {|test "t" { c: nope {} expect c: ok }|};
    tc "missing call input (TC0059)" [ "TC0059" ]
      {|test "t" { c: client {} got: c.get_user() expect c: ok }|};
    tc "ctor missing required field (TC0059)" [ "TC0059" ]
      {|test "t" { c: client {} got: c.get_user(user_ref {}) expect c: ok }|};
    tc "pattern field typo (TC0060)" [ "TC0060" ]
      {|test "t" { c: client {} got: c.get_user(user_ref { username: "x" }) expect got: user { nope: any, .. } }|};
    tc "None on a required field (TC0060)" [ "TC0060" ]
      {|test "t" { c: client {} got: c.get_user(user_ref { username: "x" }) expect got: user { login: None, .. } }|};
    tc "ok on an op with output (TC0060)" [ "TC0060" ]
      {|test "t" { c: client {} got: c.get_user(user_ref { username: "x" }) expect got: ok }|};
    tc "pattern off the op surface (TC0060)" [ "TC0060" ]
      {|test "t" { c: client {} got: c.get_user(user_ref { username: "x" }) expect got: note { id: "x", .. } }|};
    tc "stub asserted without .requests (TC0060)" [ "TC0060" ]
      {|test "t" { c: client {} s: stub c.get_user.http: http.response { status: 200 } expect s: user { .. } }|};
    tc "no expect (TC0061)" [ "TC0061" ] {|test "t" { c: client {} }|};
    tc ".requests on a call binding (TC0062)" [ "TC0062" ]
      {|test "t" { c: client {} got: c.get_user(user_ref { username: "x" }) expect got.requests: [http.request { .. }] }|};
    tc "duplicate binding (TC0066)" [ "TC0066" ]
      {|test "t" { c: client {} c: client {} expect c: ok }|};
  ]

(* ── "ext"/"extern" FFI stub forms ───────────────────────────────────────
   A free function ("companyconfig.load") and an opaque-handle method
   ("companybus.publisher.send", qualified by its type) declared alongside
   the ordinary base fixture, so a declared test can stub either shape.
   [overloaded] (already declared by [base] for [save_note]'s error) is
   reused as the method's declared error sentinel. *)
let ext_base =
  with_base
    {|
ext companyconfig {
  go: "github.com/company/config"
  ts: "@company/config"

  struct go_config { Host: string }

  extern load(service: string): app_config {
    go {
      call: "Load"(service)
      yields: (cfg: go_config)
      returns: app_config { endpoint: .cfg.Host }
    }
    ts {
      call: "load"(service)
      yields: (cfg: go_config)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

ext companybus {
  go: "github.com/company/bus"
  ts: "@company/bus"

  struct go_ack { ID: string }

  type publisher {
    extern send(topic: string): ack {
      go {
        call: "Send"(topic)
        yields: (a: go_ack)
        returns: ack { id: .a.ID }
        errors: { "ErrBusy" => overloaded }
      }
      ts {
        call: "send"(topic)
        yields: (a: go_ack)
        returns: ack { id: .a.ID }
        errors: { "BUSY" => overloaded }
      }
    }

    extern count(): i32 {
      go { call: "Count"() }
      ts { call: "count"() }
    }
  }

  extern connect(endpoint: string): publisher {
    go { call: "Connect"(endpoint) }
    ts { call: "connect"(endpoint) }
  }
}

struct app_config { endpoint: string }
pub struct ack { id: string }
|}

let etc name expected body =
  Alcotest.test_case name `Quick (fun () ->
      Alcotest.(check (list string)) name expected (codes (ext_base ^ body)))

let extern_stub_cases =
  [
    etc "free extern stub happy path" []
      {|test "t" {
  c: client { api_token: "t0" }
  stub companyconfig.load: app_config { endpoint: "https://stub" }
  expect c: ok
}|};
    etc "handle method stub happy path (value)" []
      {|test "t" {
  c: client { api_token: "t0" }
  stub companybus.publisher.send: ack { id: "a1" }
  expect c: ok
}|};
    etc "handle method stub happy path (declared error)" []
      {|test "t" {
  c: client { api_token: "t0" }
  stub companybus.publisher.send: overloaded { message: "busy" }
  expect c: ok
}|};
    etc "free extern stub wrong shape (TC0059)" [ "TC0059" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub companyconfig.load: ack { id: "x" }
  expect c: ok
}|};
    etc "handle method stub wrong shape (TC0059)" [ "TC0059" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub companybus.publisher.send: app_config { endpoint: "x" }
  expect c: ok
}|};
    etc "unknown extern function (TC0057)" [ "TC0057" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub companyconfig.ghost: app_config { endpoint: "x" }
  expect c: ok
}|};
    etc "unknown extern lib (TC0057)" [ "TC0057" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub ghostlib.ghost: app_config { endpoint: "x" }
  expect c: ok
}|};
    etc "unknown opaque method (TC0057)" [ "TC0057" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub companybus.publisher.ghost: ack { id: "x" }
  expect c: ok
}|};
    etc "unknown opaque type (TC0057)" [ "TC0057" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub companybus.nope.send: ack { id: "x" }
  expect c: ok
}|};
    etc "construction stub missing dependency segment (TC0057)" [ "TC0057" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub c.get_user: user { login: "x", id: 1 }
  expect c: ok
}|};
    etc "extern free stub binding has no outcome to assert (TC0060)"
      [ "TC0060" ]
      {|test "t" {
  c: client { api_token: "t0" }
  s: stub companyconfig.load: app_config { endpoint: "https://stub" }
  expect s: ok
  expect c: ok
}|};
    etc "extern method stub binding has no outcome to assert (TC0060)"
      [ "TC0060" ]
      {|test "t" {
  c: client { api_token: "t0" }
  s: stub companybus.publisher.send: ack { id: "a1" }
  expect s: ok
  expect c: ok
}|};
    etc "extern method stub answered with a non-constructor value" []
      {|test "t" {
  c: client { api_token: "t0" }
  stub companybus.publisher.count: 42
  expect c: ok
}|};
    etc "extern free stub empty sequence (TC0058)" [ "TC0058" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub companyconfig.load: []
  expect c: ok
}|};
    etc "extern method stub answered with a sequence" []
      {|test "t" {
  c: client { api_token: "t0" }
  stub companybus.publisher.send: [ack { id: "a1" }, ack { id: "a2" }]
  expect c: ok
}|};
    etc "extern method stub answered with an unknown constructor (TC0058)"
      [ "TC0058" ]
      {|test "t" {
  c: client { api_token: "t0" }
  stub companybus.publisher.send: nope.thing { x: 1 }
  expect c: ok
}|};
  ]

(* A shape that is both output and declared error is unreadable (TC0063). *)
let ambiguous_shape () =
  let src =
    {|
@status(500) @errorCode("code", "boom")
pub struct boom { message: string }
pub struct client {
  endpoint: string @env("EP") @default("https://x")
  op run(boom): boom @errors(boom)
    @http(method: "POST", path: "/run", endpoint: .endpoint)
}
test "t" {
  c: client {}
  got: c.run(boom { message: "x" })
  expect got: boom { .. }
}
|}
  in
  Alcotest.(check bool)
    "TC0063 reported" true
    (List.mem "TC0063"
       (List.filter_map (fun (d : Diagnostic.t) -> d.code) (snd (compile src))))

(* http.*/errors.* without the import suggests the import line (TC0064). *)
let import_missing () =
  let src =
    {|
pub struct user { login: string }
pub struct user_ref { username: string }
pub struct client {
  endpoint: string @env("EP") @default("https://x")
  op get_user(user_ref: user_ref): user
    @http(method: "GET", path: "/u/{.user_ref.username}", endpoint: .endpoint)
}
test "t" {
  c: client {}
  stub c.get_user.http: http.response { status: 200 }
  got: c.get_user(user_ref { username: "x" })
  expect got: errors.api { .. }
}
|}
  in
  let ds = snd (compile src) in
  Alcotest.(check (list string))
    "two import hints" [ "TC0064"; "TC0064" ]
    (List.filter_map (fun (d : Diagnostic.t) -> d.code) ds);
  Alcotest.(check bool)
    "suggests the import line" true
    (List.exists
       (fun (d : Diagnostic.t) ->
         d.code = Some "TC0064" && contains d.message "import tono.http")
       ds)

(* ── The tono.* module root (project pipeline) ─────────────────────────── *)

let project_codes files =
  List.filter_map
    (fun (d : Diagnostic.t) -> d.code)
    (snd (Tono_frontend.compile_project files))

let reserved_root () =
  Alcotest.(check (list string))
    "reserved root" [ "TC0065" ]
    (project_codes [ ("tono.util", "pub struct x { a: string }") ])

let virtual_imports_resolve () =
  Alcotest.(check (list string))
    "no unknown-module code" []
    (project_codes
       [
         ( "app",
           "import tono.http\nimport tono.errors\npub struct a { x: string }" );
       ])

let unknown_language_module () =
  Alcotest.(check (list string))
    "unknown tono.* import" [ "TC0023" ]
    (project_codes [ ("app", "import tono.magic\npub struct a { x: string }") ])

(* ── fmt ───────────────────────────────────────────────────────────────── *)

let fmt src =
  let file, ds = Parser.parse src in
  Alcotest.(check int) "parses cleanly" 0 (List.length (errors_of ds));
  Printer.print_file file

let fmt_golden () =
  let src =
    "import tono.http\n\
     test \"a messy test\" { c: client {   } , s: stub c.get.http: \
     http.response { status: 200,, body: \"{}\" }\n\
     got: c.get(user_ref { username: \"x\" })\n\
     expect got: user { .. , login: \"x\", bio: None, name: any }\n\
     expect s.requests: [http.request { method: \"GET\", headers: { \"a-b\": \
     \"c\", .. } , .. }] }"
  in
  let expected =
    "import tono.http\n\n\
     test \"a messy test\" {\n\
    \  c: client {}\n\
    \  s: stub c.get.http: http.response { status: 200, body: \"{}\" }\n\
    \  got: c.get(user_ref { username: \"x\" })\n\
    \  expect got: user { login: \"x\", bio: None, name: any, .. }\n\
    \  expect s.requests: [http.request { method: \"GET\", headers: { \"a-b\": \
     \"c\", .. }, .. }]\n\
     }\n"
  in
  Alcotest.(check string) "canonical layout" expected (fmt src)

let fmt_idempotent () =
  let once = fmt rich_src in
  Alcotest.(check string) "fmt (fmt x) = fmt x" once (fmt once)

(* Both new stub forms print as a plain dotted path and re-parse to the same
   text, exactly like the existing binding.op.dep form. *)
let extern_stub_fmt_roundtrip () =
  let src =
    ext_base
    ^ {|
test "t" {
  c: client { api_token: "t0" }
  stub companyconfig.load: app_config { endpoint: "https://stub" }
  stub companybus.publisher.send: ack { id: "a1" }
  expect c: ok
}|}
  in
  let once = fmt src in
  Alcotest.(check string) "fmt (fmt x) = fmt x" once (fmt once);
  Alcotest.(check bool)
    "free-fn path prints" true
    (contains once "stub companyconfig.load:");
  Alcotest.(check bool)
    "method path prints" true
    (contains once "stub companybus.publisher.send:")

let () =
  Alcotest.run "declared_tests"
    [
      ( "lowering",
        [
          Alcotest.test_case "rich file compiles clean" `Quick
            rich_compiles_clean;
          Alcotest.test_case "lowered shape" `Quick lowered_shape;
          Alcotest.test_case "impl answers and dataflow" `Quick
            impl_and_dataflow;
          Alcotest.test_case "i64 wire form" `Quick wire_i64;
          Alcotest.test_case "round-trip" `Quick roundtrip;
          Alcotest.test_case "tests key always written" `Quick
            tests_key_always_written;
        ] );
      ( "parser",
        [
          Alcotest.test_case "rejects malformed tests" `Quick parser_rejects;
          Alcotest.test_case "contextual words stay identifiers" `Quick
            contextual_words_stay_identifiers;
        ] );
      ( "typecheck",
        binding_cases @ extern_stub_cases
        @ [
            Alcotest.test_case "ambiguous output/error shape" `Quick
              ambiguous_shape;
            Alcotest.test_case "missing language import" `Quick import_missing;
            Alcotest.test_case "reserved tono.* root" `Quick reserved_root;
            Alcotest.test_case "virtual imports resolve" `Quick
              virtual_imports_resolve;
            Alcotest.test_case "unknown language module" `Quick
              unknown_language_module;
          ] );
      ( "fmt",
        [
          Alcotest.test_case "golden layout" `Quick fmt_golden;
          Alcotest.test_case "idempotent" `Quick fmt_idempotent;
          Alcotest.test_case "extern stub round-trip" `Quick
            extern_stub_fmt_roundtrip;
        ] );
    ]
