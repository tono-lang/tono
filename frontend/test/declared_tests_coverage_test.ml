open Tono_frontend

(* Coverage companion to declared_tests_test: each case targets a specific
   diagnostic arm, encoder combination, pattern form, parser rejection, or
   codec branch of the declared-test pipeline. *)

(* ── Helpers ───────────────────────────────────────────────────────────── *)

let errors_of ds =
  List.filter (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) ds

let compile src = Tono_frontend.compile ~module_name:"github" src

let codes src =
  List.filter_map (fun (d : Diagnostic.t) -> d.code) (snd (compile src))

let count_code c src = List.length (List.filter (String.equal c) (codes src))

(* A base whose value surface spans every encoder branch: enums (string- and
   int-backed), a union, generics, lists, maps, nullable members, nested
   structs, i64/u64/float/bool prims, and ops with struct, prim, enum, and no
   output over both http and impl dependencies. *)
let vbase =
  {|
import tono.http
import tono.errors

pub enum color { red, green }
pub enum level { low = 1, high = 2 }

pub struct circle { r: i32 }
pub union blob { one(circle) }

pub struct box[t] { v: t }

pub struct addr {
  city: string
  zip: string?
}

pub struct person {
  name: string
  age: u32?
  big: i64?
  huge: u64?
  rate: float?
  flag: bool?
  tags: []string?
  attrs: map[string]string?
  favorite: color?
  lvl: level?
  home: addr?
  blobf: blob?
  gen: box[string]?
}

pub struct person_ref { key: string @httpLabel }

@status(500) @errorCode("code", "boom")
pub struct boom { message: string }

pub struct vclient {
  endpoint: string @env("EP") @default("https://x")

  op get(person_ref): person
    @http(method: "GET", path: "/p/{key}", endpoint: .endpoint)

  op put(person): person @errors(boom)

  op word(person_ref): string
    @http(method: "GET", path: "/w/{key}", endpoint: .endpoint)

  op tag(): color
    @http(method: "GET", path: "/t", endpoint: .endpoint)

  op nop()
    @http(method: "GET", path: "/n", endpoint: .endpoint)
}

ext impl put {
  go: "ext/go/p.go#Put"
}
|}

let with_vbase body = vbase ^ "\n" ^ body

let counts name expected body =
  Alcotest.test_case name `Quick (fun () ->
      let src = with_vbase body in
      List.iter
        (fun (code, n) ->
          Alcotest.(check int)
            (Printf.sprintf "%s x%d" code n)
            n (count_code code src))
        expected)

(* ── Value encoding, the happy path ────────────────────────────────────── *)

let values_src =
  with_vbase
    {|
test "values" {
  c: vclient {}
  saved: c.put(person {
    name: "n", age: 3, big: 5, huge: 7, rate: 1.5, flag: true,
    tags: ["a"], attrs: { k: "v" }, favorite: "red", lvl: "low",
    home: addr { city: "x" }
  })
  expect saved: person { name: "n", .. }
  again: c.put(person { name: saved.home.city, rate: 2 })
  expect again: person { .. }
}

test "requests" {
  c: vclient {}
  s: stub c.get.http: http.response { status: 200 }
  got: c.get(person_ref { key: "k" })
  expect s.requests: [http.request { method: "GET" }]
}
|}

let encoded_values () =
  let m, ds = compile values_src in
  Alcotest.(check (list string))
    "clean" []
    (List.map Diagnostic.to_string (errors_of ds));
  match (List.hd m.Ir.tests).Ir.t_calls with
  | [
   { Ir.call_input = Some (`Assoc kvs); _ };
   { Ir.call_input = Some (`Assoc kvs2); _ };
  ] ->
      let eq name expected =
        Alcotest.(check bool) name true (List.assoc name kvs = expected)
      in
      eq "age" (`Int 3);
      eq "big" (`String "5");
      eq "huge" (`String "7");
      eq "rate" (`Float 1.5);
      eq "flag" (`Bool true);
      eq "tags" (`List [ `String "a" ]);
      eq "attrs" (`Assoc [ ("k", `String "v") ]);
      eq "favorite" (`String "red");
      eq "lvl" (`Int 1);
      eq "home" (`Assoc [ ("city", `String "x") ]);
      Alcotest.(check bool)
        "int fills float" true
        (List.assoc "rate" kvs2 = `Float 2.0);
      Alcotest.(check bool)
        "multi-segment ref" true
        (List.assoc "name" kvs2
        = `Assoc
            [
              ( "$ref",
                `Assoc
                  [
                    ("binding", `String "saved");
                    ("path", `List [ `String "home"; `String "city" ]);
                  ] );
            ])
  | _ -> Alcotest.fail "two calls expected"

(* ── Typecheck diagnostic arms ─────────────────────────────────────────── *)

let value_mismatch_cases =
  [
    (* One mismatch per encoder branch: prim kinds, list, map, enum value and
       kind, struct, union, and generics. *)
    counts "value type mismatches"
      [ ("TC0059", 13) ]
      {|test "t" {
  c: vclient {}
  x: c.put(person {
    name: 1.5, age: true, big: [1], huge: { a: 1 }, rate: "x",
    flag: addr { city: "x" }, tags: "x", attrs: 5, favorite: "blue",
    lvl: 5, home: 5, blobf: 5, gen: 5
  })
  expect x: person { .. }
}|};
    counts "constructor head mismatches"
      [ ("TC0059", 6) ]
      {|test "t" {
  c: vclient {}
  x: c.put(person { name: "n", home: person { name: "z" } })
  y: c.put(person { name: "n", home: http.response { status: 200 } })
  z: c.put(person { name: "n", home: zz.ww {} })
  w: c.put(person { name: "n", home: a.b.c {} })
  v: c.put(person { name: "n", home: addr { bogus: "x" } })
  expect x: person { .. }
}|};
    counts "dataflow reference arms"
      [ ("TC0059", 5); ("TC0055", 1) ]
      {|test "t" {
  c: vclient {}
  saved: c.put(person { name: "n" })
  done: c.nop()
  a: c.put(person { name: saved.bogus })
  b: c.put(person { name: saved.favorite.x })
  d: c.put(person { name: saved.name.x })
  e: c.put(person { name: ghost.name })
  f: c.put(person { name: c.name })
  g: c.put(person { name: done.x })
  expect saved: person { .. }
}|};
    (* The non-integer status reports twice: once for the type, once for the
       still-missing status. *)
    counts "http stub answer arms"
      [ ("TC0058", 9) ]
      {|test "t" {
  c: vclient {}
  stub c.get.http: http.response { status: "x" }
  stub c.get.http: http.response { status: 200, headers: { a: 1 } }
  stub c.get.http: http.response { status: 200, headers: "x" }
  stub c.get.http: http.response { status: 200, body: 1 }
  stub c.get.http: http.response { status: 200, bogus: 1 }
  stub c.get.http: http.response { body: "x" }
  stub c.get.http: 5
  stub c.get.http: ,
  expect c: ok
}|};
    counts "impl stub answer arms"
      [ ("TC0058", 4) ]
      {|test "t" {
  c: vclient {}
  stub c.put.impl: errors.contract { name: "x" }
  stub c.put.impl: zz.ww {}
  stub c.put.impl: http.response { status: 200 }
  stub c.put.impl: 5
  expect c: ok
}|};
    counts "construction pattern arms"
      [ ("TC0060", 6) ]
      {|test "t" {
  c: vclient {}
  expect c: errors.api { .. }
  expect c: errors.bogus {}
  expect c: zz.ww {}
  expect c: person { .. }
  expect c: [1]
  expect c: { a: 1 }
  expect c: errors.config { field: "endpoint" }
  expect c: errors.validation { fields: ["e"], .. }
}|};
    counts "call pattern arms"
      [ ("TC0060", 2) ]
      {|test "t" {
  c: vclient {}
  got: c.get(person_ref { key: "k" })
  expect got: http.request { .. }
  t: c.tag()
  expect t: color {}
  w: c.word(person_ref { key: "k" })
  expect w: "hello"
  done: c.nop()
  expect done: 5
}|};
    counts "field pattern arms"
      [ ("TC0060", 9) ]
      {|test "t" {
  c: vclient {}
  got: c.get(person_ref { key: "k" })
  expect got: person { home: addr { city: "x", zip: None, .. }, .. }
  expect got: person { home: person { .. }, .. }
  expect got: person { home: errors.api { .. }, .. }
  expect got: person { home: zz.ww {}, .. }
  expect got: person { favorite: color {}, .. }
  expect got: person { name: addr { .. }, .. }
  expect got: person { home: ok, .. }
  expect got: person { tags: ["a"], attrs: { k: "v" }, .. }
  expect got: person { tags: [ok], .. }
  expect got: person { attrs: { k: any }, .. }
  expect got: person { attrs: { k: "v", .. }, .. }
}|};
    counts "request pattern arms"
      [ ("TC0060", 7) ]
      {|test "t" {
  c: vclient {}
  s: stub c.get.http: http.response { status: 200 }
  s2: stub c.put.impl: errors.contract {}
  got: c.get(person_ref { key: "k" })
  expect s2.requests: [http.request { .. }]
  expect s.requests: http.request { .. }
  expect s.requests: ok
  expect s.requests: [person {}, zz.ww {}, 5,
    http.request { headers: "x" },
    http.request { method: "GET", headers: { "a": any, "b": None, .. }, .. }]
}|};
    counts "item binding arms"
      [ ("TC0059", 2); ("TC0056", 1); ("TC0055", 2) ]
      {|test "t" {
  p: person { name: "n" }
  c: vclient {}
  stub c.ghost.http: http.response { status: 200 }
  s: stub c.get.http: http.response { status: 200 }
  stub s.get.http: http.response { status: 200 }
  done: c.nop(zz)
  got: ghost.get(person_ref { key: "k" })
  expect c: ok
}|};
    counts "error-typed pattern leaves"
      [ ("TC0060", 1) ]
      {|test "t" {
  c: vclient {}
  got: c.get(person_ref { key: "k" })
  expect got: ,
  done: c.nop()
  expect done: ,
}|};
    counts "parse-error value encodes silently" []
      {|test "t" {
  c: vclient {}
  x: c.put(person { name: , })
  expect x: person { .. }
}|};
  ]

(* A shape that is both output and declared error is as unreadable in an impl
   stub answer as it is in a pattern. *)
let ambiguous_stub_answer () =
  let src =
    {|
@status(500) @errorCode("code", "kapow")
pub struct kapow { message: string }
pub struct amb {
  endpoint: string @env("EP2") @default("https://x")
  op run(kapow): kapow @errors(kapow)
}
ext impl run {
  go: "ext/go/a.go#Run"
}
test "t" {
  c: amb {}
  stub c.run.impl: kapow { message: "x" }
  expect c: ok
}
|}
  in
  Alcotest.(check int) "TC0063 reported" 1 (count_code "TC0063" src)

(* A member whose type did not parse (TError) or resolve encodes to null
   without a cascading value diagnostic. *)
let unparsed_member_type () =
  let src =
    {|
pub struct wk { a: ? }
pub struct ec {
  endpoint: string @env("EP3") @default("https://x")
  op send(wk): wk
    @http(method: "POST", path: "/s", endpoint: .endpoint)
}
test "t" {
  c: ec {}
  x: c.send(wk { a: 5 })
  expect x: wk { .. }
}
|}
  in
  let _, ds = compile src in
  Alcotest.(check bool) "parse errors reported" true (errors_of ds <> []);
  Alcotest.(check int) "no value diagnostic" 0 (count_code "TC0059" src)

let unresolved_member_type () =
  let src =
    {|
pub struct un { u: ghosttype }
pub struct ec {
  endpoint: string @env("EP4") @default("https://x")
  op send(un): un
    @http(method: "POST", path: "/s", endpoint: .endpoint)
}
test "t" {
  c: ec {}
  x: c.send(un { u: 5 })
  expect x: un { .. }
}
|}
  in
  Alcotest.(check bool)
    "unresolved type reported" true
    (List.mem "TC0001" (codes src));
  Alcotest.(check int) "no value diagnostic" 0 (count_code "TC0059" src)

(* ── Parser arms ───────────────────────────────────────────────────────── *)

let parse_errors src =
  let _, ds = Parser.parse src in
  List.length (errors_of ds)

let parser_accepts () =
  let cases =
    [
      ( "stub value kinds",
        {|test "t" { stub c.o.http: "x" stub c.o.http: 1.5 stub c.o.http: true stub c.o.http: someref stub c.o.http: { a: 1 } }|}
      );
      ("pattern literal kinds", {|test "t" { expect g: 1.5 expect g: true }|});
      ("value map comma skip", {|test "t" { stub c.o.http: { , a: 1 } }|});
      ("pattern list comma skip", {|test "t" { expect s.requests: [ , ok ] }|});
      ("pattern fields comma skip", {|test "t" { expect g: u { , a: any } }|});
      ("map pattern comma skip", {|test "t" { expect g: { , a: any } }|});
    ]
  in
  List.iter
    (fun (name, src) -> Alcotest.(check int) name 0 (parse_errors src))
    cases

let parser_rejects_more () =
  let cases =
    [
      ("None in a value", {|test "t" { c: cl { a: None } }|});
      ("bad map key in a value", {|test "t" { c: cl { a: { 1: 2 } } }|});
      ("no value after ':'", {|test "t" { c: cl { a: : } }|});
      ("unclosed value list", {|test "t" { stub c.o.http: [1, 2|});
      ("unclosed value map", {|test "t" { stub c.o.http: { a: 1|});
      ("stray token in ctor body", {|test "t" { c: cl { : } }|});
      ("unclosed ctor", {|test "t" { c: cl { a: 1|});
      ("stub value parse error", {|test "t" { stub c.o.http: , }|});
      ("no pattern after ':'", {|test "t" { expect g: , }|});
      ("unclosed pattern list", {|test "t" { expect s.requests: [ok|});
      ("stray token in pattern body", {|test "t" { expect g: u { : } }|});
      ("unclosed pattern fields", {|test "t" { expect g: u { a: any|});
      ("bad map pattern key", {|test "t" { expect g: { 1: any } }|});
      ("unclosed map pattern", {|test "t" { expect g: { "a": any|});
      ("unclosed call input", {|test "t" { g: c.op("x"|});
      ("name then junk", {|test "t" { c: cl }|});
      ("literal binding rhs", {|test "t" { c: 5 }|});
      ("unclosed test body", {|test "t" { c: cl {}|});
      ("literal test item", {|test "t" { 5 }|});
    ]
  in
  List.iter
    (fun (name, src) -> Alcotest.(check bool) name true (parse_errors src > 0))
    cases

(* ── JSON codec arms ───────────────────────────────────────────────────── *)

(* [Ir_json_tests] is not re-exported by [Tono_frontend], so every codec arm
   is reached through the module codec: a well-formed module template carries
   one crafted test, and [Ir_json.decode_module] surfaces the branch. *)

let j = Yojson.Safe.from_string

let module_template =
  lazy
    (let m, _ = compile "pub struct a { x: string }" in
     Ir_json.encode_module m)

let with_tests (tests : Ir.json) : Ir.json =
  match Lazy.force module_template with
  | `Assoc kvs ->
      `Assoc
        (List.map
           (fun (k, v) -> if String.equal k "tests" then (k, tests) else (k, v))
           kvs)
  | _ -> Alcotest.fail "the module template must encode to an object"

let decode_one_test name (t : string) : Ir.test_decl =
  match Ir_json.decode_module (with_tests (`List [ j t ])) with
  | Ok m -> List.hd m.Ir.tests
  | Error e -> Alcotest.failf "%s: %s" name e

let decode_rejections () =
  let reject name t =
    match Ir_json.decode_module (with_tests (`List [ j t ])) with
    | Error _ -> ()
    | Ok _ -> Alcotest.failf "%s: an error was expected" name
  in
  reject "test name" "{}";
  reject "construction binding" {|{"name":"t","constructions":[{}]}|};
  reject "construction entry" {|{"name":"t","constructions":[{"binding":"b"}]}|};
  reject "stub client" {|{"name":"t","stubs":[{}]}|};
  reject "stub dep" {|{"name":"t","stubs":[{"client":"c","op":"o"}]}|};
  reject "unknown dep"
    {|{"name":"t","stubs":[{"client":"c","op":"o","dep":"tcp"}]}|};
  reject "answer shape"
    {|{"name":"t","stubs":[{"client":"c","op":"o","dep":"http",
       "answers":[{"error":{"data":{}}}]}]}|};
  reject "answer kind"
    {|{"name":"t","stubs":[{"client":"c","op":"o","dep":"http",
       "answers":[{"bogus":1}]}]}|};
  reject "call binding" {|{"name":"t","calls":[{}]}|};
  reject "pattern shape"
    {|{"name":"t","expects":[{"subject":"s","pattern":{"struct":{}}}]}|};
  reject "pattern kind"
    {|{"name":"t","expects":[{"subject":"s","pattern":{"bogus":1}}]}|};
  reject "field pattern"
    {|{"name":"t","expects":[{"subject":"s",
       "pattern":{"struct":{"shape":"s","fields":{"f":{"zz":1}}}}}]}|};
  reject "expect subject" {|{"name":"t","expects":[{}]}|};
  reject "expect payload" {|{"name":"t","expects":[{"subject":"s"}]}|}

let decode_defaults () =
  let t = decode_one_test "empty test" {|{"name":"t"}|} in
  Alcotest.(check bool)
    "empty sections" true
    (t.Ir.t_constructions = [] && t.Ir.t_stubs = [] && t.Ir.t_calls = []
   && t.Ir.t_expects = []);
  let t =
    decode_one_test "construction"
      {|{"name":"t","constructions":[{"binding":"b","entry":"e"}]}|}
  in
  (match t.Ir.t_constructions with
  | [ c ] -> Alcotest.(check bool) "values default" true (c.Ir.tc_values = [])
  | _ -> Alcotest.fail "one construction expected");
  let t =
    decode_one_test "stub answers"
      {|{"name":"t","stubs":[{"client":"c","op":"o","dep":"impl",
         "answers":[{"status":200},{"error":{"shape":"s"}}]}]}|}
  in
  (match t.Ir.t_stubs with
  | [ { Ir.ts_binding = None; ts_dep = Ir.Dep_impl; ts_answers = [ a; b ]; _ } ]
    -> (
      (match a with
      | Ir.Answer_http { ans_status; ans_headers; ans_body } ->
          Alcotest.(check int) "status" 200 ans_status;
          Alcotest.(check bool) "headers default" true (ans_headers = []);
          Alcotest.(check string) "body default" "" ans_body
      | _ -> Alcotest.fail "http answer expected");
      match b with
      | Ir.Answer_error { ans_shape; ans_data } ->
          Alcotest.(check string) "shape" "s" ans_shape;
          Alcotest.(check bool) "data default" true (ans_data = `Assoc [])
      | _ -> Alcotest.fail "error answer expected")
  | _ -> Alcotest.fail "one stub expected");
  let t =
    decode_one_test "empty stub answers"
      {|{"name":"t","stubs":[{"client":"c","op":"o","dep":"http"}]}|}
  in
  (match t.Ir.t_stubs with
  | [ s ] -> Alcotest.(check bool) "answers default" true (s.Ir.ts_answers = [])
  | _ -> Alcotest.fail "one stub expected");
  let t =
    decode_one_test "null call input"
      {|{"name":"t","calls":[{"binding":"b","client":"c","op":"o","input":null}]}|}
  in
  (match t.Ir.t_calls with
  | [ c ] -> Alcotest.(check bool) "null input" true (c.Ir.call_input = None)
  | _ -> Alcotest.fail "one call expected");
  let t =
    decode_one_test "pattern defaults"
      {|{"name":"t","expects":[{"subject":"s",
         "pattern":{"struct":{"shape":"s"}}}]}|}
  in
  (match t.Ir.t_expects with
  | [ Ir.Expect_outcome { ex_pattern = Ir.P_struct p; _ } ] ->
      Alcotest.(check bool) "open default" false p.ps_open;
      Alcotest.(check bool) "fields default" true (p.ps_fields = [])
  | _ -> Alcotest.fail "one struct-pattern expect expected");
  let t =
    decode_one_test "request defaults"
      {|{"name":"t","expects":[{"subject":"s","requests":[{}]}]}|}
  in
  match t.Ir.t_expects with
  | [ Ir.Expect_requests { ex_requests = [ rp ]; _ } ] ->
      Alcotest.(check bool)
        "request defaults" true
        ((not rp.Ir.rp_open) && rp.Ir.rp_fields = [] && rp.Ir.rp_headers = None)
  | _ -> Alcotest.fail "one requests expect expected"

(* A '.requests' expect whose patterns cite no headers lowers with
   [rp_headers = None] and encodes without a headers key. *)
let headerless_request () =
  let m, ds = compile values_src in
  Alcotest.(check int) "clean" 0 (List.length (errors_of ds));
  match (List.nth m.Ir.tests 1).Ir.t_expects with
  | [ Ir.Expect_requests { ex_requests = [ rp ]; _ } ] ->
      Alcotest.(check bool) "no headers" true (rp.Ir.rp_headers = None)
  | _ -> Alcotest.fail "one requests expect expected"

(* A full round-trip over the coverage base keeps both codec directions on the
   richer answer and pattern shapes honest. *)
let roundtrip_values () =
  let m, ds = compile values_src in
  Alcotest.(check int) "clean" 0 (List.length (errors_of ds));
  let json = Ir_json.encode_module m in
  match Ir_json.decode_module json with
  | Error e -> Alcotest.failf "module did not round-trip: %s" e
  | Ok m' ->
      Alcotest.(check string)
        "re-encode is identical"
        (Ir_json.to_canonical_string json)
        (Ir_json.to_canonical_string (Ir_json.encode_module m'))

let () =
  Alcotest.run "declared_tests_coverage"
    [
      ( "values",
        [
          Alcotest.test_case "encoded values" `Quick encoded_values;
          Alcotest.test_case "round-trip" `Quick roundtrip_values;
        ] );
      ( "typecheck",
        value_mismatch_cases
        @ [
            Alcotest.test_case "ambiguous impl stub answer" `Quick
              ambiguous_stub_answer;
            Alcotest.test_case "unparsed member type" `Quick
              unparsed_member_type;
            Alcotest.test_case "unresolved member type" `Quick
              unresolved_member_type;
          ] );
      ( "parser",
        [
          Alcotest.test_case "accepts value and pattern forms" `Quick
            parser_accepts;
          Alcotest.test_case "rejects malformed forms" `Quick
            parser_rejects_more;
        ] );
      ( "codec",
        [
          Alcotest.test_case "decode rejections" `Quick decode_rejections;
          Alcotest.test_case "decode defaults" `Quick decode_defaults;
          Alcotest.test_case "headerless request" `Quick headerless_request;
        ] );
    ]
