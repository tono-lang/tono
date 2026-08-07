open Tono_frontend

let errors_of ds =
  List.filter (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) ds

(* Parse asserting no errors, then print. *)
let fmt src =
  let file, ds = Parser.parse src in
  Alcotest.(check int) "parses cleanly" 0 (List.length (errors_of ds));
  Printer.print_file file

(* A deliberately messy file covering every construct, checked against the one
   canonical layout the printer emits. *)
let golden_layout () =
  let src =
    {|
@doc("payments")   pub struct charge { id: uuid, amount_cents: i64 @range(min: 0), note: string?
  tags: []string meta: map[string]string? }
enum currency { usd eur }
enum http_code { ok = 200, error = 500 }
@discriminator("kind") union source[t] { card(card) @doc("plastic"), bank(bank_account), page(page[t]) }
struct empty {}
op create_charge(charge): charge @errors(not_found, conflict) @http(method: "post")
op ping()
|}
  in
  let expected =
    {|@doc("payments")
pub struct charge {
  id: uuid
  amount_cents: i64 @range(min: 0)
  note: string?
  tags: []string
  meta: map[string]string?
}

enum currency {
  usd
  eur
}

enum http_code {
  ok = 200
  error = 500
}

@discriminator("kind")
union source[t] {
  card(card) @doc("plastic")
  bank(bank_account)
  page(page[t])
}

struct empty {}

op create_charge(charge): charge
  @errors(not_found, conflict)
  @http(method: "post")

op ping()
|}
  in
  Alcotest.(check string) "canonical layout" expected (fmt src)

(* Formatting is a fixpoint: formatting already-formatted output is identity. *)
let idempotent () =
  let src =
    {|
@doc("payments") pub struct charge { id: uuid, note: string? }
enum currency { usd, eur }
union source { card(card), bank(bank_account) }
op create_charge(charge): charge @errors(not_found)
|}
  in
  let once = fmt src in
  Alcotest.(check string) "fmt (fmt src) = fmt src" once (fmt once)

(* Formatting preserves meaning: the formatted source compiles (lower and
   typecheck included) to the same IR. *)
let ir_equivalent () =
  let src =
    {|
@doc("payments") pub struct charge {
  id: uuid
  amount_cents: i64 @range(min: 0)
  note: string?
}
enum currency { usd, eur }
enum http_code { ok = 200, error = 500 }
struct card { last4: string }
struct bank_account { iban: string }
@discriminator("kind")
union source { card(card), bank(bank_account) }
struct page[t] { items: []t, next: string? }
@status(404) struct not_found { message: string }
op create_charge(charge): charge @errors(not_found)
|}
  in
  let m1, d1 = Tono_frontend.compile ~module_name:"payments" src in
  Alcotest.(check int) "source compiles" 0 (List.length (errors_of d1));
  let m2, d2 = Tono_frontend.compile ~module_name:"payments" (fmt src) in
  Alcotest.(check int) "formatted compiles" 0 (List.length (errors_of d2));
  Alcotest.(check string)
    "same IR"
    (Ir_json.to_canonical_string (Ir_json.encode_module m1))
    (Ir_json.to_canonical_string (Ir_json.encode_module m2))

(* Every extension kind prints to the one canonical layout, and re-parsing
   that layout yields the same IR (printer <-> parser round-trip over ext). *)
let ext_layout () =
  let src =
    {|
ext hook before_request { ts: "ext/ts/auth.ts#addBearer"  rust: "ext/rust/a.rs#f" }
ext contract   sign_request  (canonical_request) -> string {
  ts: "ext/ts/sign.ts#signRequest" conformance: "vectors/sign.json" }
ext constraint luhn (string) -> bool { ts: "ext/ts/luhn.ts#isLuhn" }
ext impl   client.save   raw { go: "ext/go/s.go#Save"  ts: "ext/ts/s.ts#save" }
|}
  in
  let expected =
    {|ext hook before_request {
  ts: "ext/ts/auth.ts#addBearer"
  rust: "ext/rust/a.rs#f"
}

ext contract sign_request (canonical_request) -> string {
  ts: "ext/ts/sign.ts#signRequest"
  conformance: "vectors/sign.json"
}

ext constraint luhn (string) -> bool {
  ts: "ext/ts/luhn.ts#isLuhn"
}

ext impl client.save raw {
  go: "ext/go/s.go#Save"
  ts: "ext/ts/s.ts#save"
}
|}
  in
  Alcotest.(check string) "canonical ext layout" expected (fmt src);
  (* Idempotent, and the formatted source compiles to the same IR. *)
  Alcotest.(check string) "idempotent" expected (fmt (fmt src));
  let m1, _ = Tono_frontend.compile ~module_name:"m" src in
  let m2, _ = Tono_frontend.compile ~module_name:"m" (fmt src) in
  Alcotest.(check string)
    "same IR"
    (Ir_json.to_canonical_string (Ir_json.encode_module m1))
    (Ir_json.to_canonical_string (Ir_json.encode_module m2))

(* The entry model in one file: stacked value sources, a derived field with a
   template and a catalog pipeline, a selection table with every arm shape, a
   composed config with its binding, and operations carrying the protocol
   vocabulary. Fields stay on one line (a source chain is short by nature);
   operations become blocks. *)
let entry_layout () =
  let src =
    {|
struct settings { api_key: string @env("API_KEY") }
pub struct client { client_name: string @arg @str::trim
  client_key: string @format("{.client_name}") @str::upper_snake
  endpoint_env: string @format("ENDPOINT_{.client_key}_V2")
  version: string @env("ENDPOINT_VERSION") @default("v2")
  endpoint_v1: string @env("ENDPOINT") endpoint_v2: string @env(.endpoint_env)
  timeout: duration @with @default("10s")
  creds: credentials @env("SERVICE_CREDENTIALS")
  endpoint: string = match .version { "v1" => .endpoint_v1
    2 => .endpoint_v2
    _ => @env("FALLBACK") @default("https://x") }
  conf: settings @bind(api_key, .client_key)
  op fetch(note_ref): note @http(method: "GET", path: "/notes/{id}", endpoint: .endpoint)
    @header("Authorization", .creds.token) @timeout(.timeout) @errors(not_found)
  op ping() }
|}
  in
  let expected =
    {|struct settings {
  api_key: string @env("API_KEY")
}

pub struct client {
  client_name: string @arg @str::trim
  client_key: string @format("{.client_name}") @str::upper_snake
  endpoint_env: string @format("ENDPOINT_{.client_key}_V2")
  version: string @env("ENDPOINT_VERSION") @default("v2")
  endpoint_v1: string @env("ENDPOINT")
  endpoint_v2: string @env(.endpoint_env)
  timeout: duration @with @default("10s")
  creds: credentials @env("SERVICE_CREDENTIALS")
  endpoint: string = match .version {
    "v1" => .endpoint_v1
    2 => .endpoint_v2
    _ => @env("FALLBACK") @default("https://x")
  }
  conf: settings @bind(api_key, .client_key)

  op fetch(note_ref): note
    @http(method: "GET", path: "/notes/{id}", endpoint: .endpoint)
    @header("Authorization", .creds.token)
    @timeout(.timeout)
    @errors(not_found)

  op ping()
}
|}
  in
  Alcotest.(check string) "canonical entry layout" expected (fmt src);
  Alcotest.(check string) "idempotent" expected (fmt (fmt src))

(* An entry whose fields and ops all round-trip through the IR: the layout
   above is not just stable text, it preserves meaning. *)
let entry_ir_equivalent () =
  let src =
    {|
struct note_ref { id: string }
struct note { id: string }
@status(404) @errorCode("code", "not_found") struct not_found { message: string }
pub struct client {
  api_key: string @arg
  endpoint: string @env("ENDPOINT") @default("https://x")
  op fetch(note_ref): note
    @http(method: "GET", path: "/notes/{id}", endpoint: .endpoint)
    @header("X-Api-Key", .api_key)
    @errors(not_found)

  op store(note): note
    @errors(not_found)
}
ext impl client.store raw { go: "ext/go/n.go#Store" }
|}
  in
  let m1, d1 = Tono_frontend.compile ~module_name:"notes" src in
  Alcotest.(check int) "source compiles" 0 (List.length (errors_of d1));
  let m2, d2 = Tono_frontend.compile ~module_name:"notes" (fmt src) in
  Alcotest.(check int) "formatted compiles" 0 (List.length (errors_of d2));
  Alcotest.(check string)
    "same IR"
    (Ir_json.to_canonical_string (Ir_json.encode_module m1))
    (Ir_json.to_canonical_string (Ir_json.encode_module m2))

(* A binding target carrying characters the literal grammar escapes still
   re-parses: the ext body is source, not a verbatim passthrough. *)
let ext_binding_escapes () =
  let target = "ext\\go\\a \"b\".go#Save" in
  let src = "ext impl save { go: " ^ Printer.string_literal target ^ " }" in
  let file, ds = Parser.parse (fmt src) in
  Alcotest.(check int) "re-parses cleanly" 0 (List.length (errors_of ds));
  match file.Ast.decls with
  | [ { Ast.dkind = Ast.DExt { ebindings = [ b ]; _ }; _ } ] ->
      Alcotest.(check string) "target survives" target b.Ast.target
  | _ -> Alcotest.fail "expected one ext declaration with one binding"

(* String and float literals re-lex to the same values. *)
let literals () =
  List.iter
    (fun s ->
      let toks, ds = Lexer.tokenize (Printer.string_literal s) in
      Alcotest.(check int) "lexes cleanly" 0 (List.length (errors_of ds));
      match toks with
      | [ { Token.kind = Token.Str s'; _ }; { Token.kind = Token.Eof; _ } ] ->
          Alcotest.(check string) "string round-trips" s s'
      | _ -> Alcotest.fail "expected a single string token")
    [ ""; "plain"; "with \"quotes\""; "line\nbreak"; "tab\tand \\ back \r" ];
  List.iter
    (fun f ->
      let lit = Printer.float_literal f in
      let toks, ds = Lexer.tokenize lit in
      Alcotest.(check int)
        ("lexes cleanly: " ^ lit) 0
        (List.length (errors_of ds));
      match toks with
      | [ { Token.kind = Token.Float f'; _ }; { Token.kind = Token.Eof; _ } ] ->
          Alcotest.(check bool)
            ("float round-trips: " ^ lit)
            true (Float.equal f f')
      | _ -> Alcotest.failf "expected a single float token for %s" lit)
    [
      0.0;
      0.5;
      -3.75;
      100.25;
      0.001;
      1e-7;
      1.5e300;
      123456789.125;
      (* infinities re-lex as overflowing literals, like the ones that made them *)
      infinity;
      neg_infinity;
    ]

(* Fallbacks for values the printer should never see from a clean parse. *)
let defensive_placeholders () =
  Alcotest.(check string) "nan" "0.0" (Printer.float_literal Float.nan);
  let dpos : Span.pos = { line = 0; col = 0; offset = 0 } in
  let dspan : Span.span = { start = dpos; finish = dpos } in
  Alcotest.(check string) "error type" "_" (Printer.print_ty (Ast.TError dspan))

(* Whitespace is not significant, so a trait between an op and the next
   declaration binds to the op. The formatter keeps op traits on the op line;
   this pins the grammar behavior the layout is designed around. *)
let op_swallows_following_traits () =
  let file, ds =
    Parser.parse {|
op ping()
@doc("next") struct s { x: i64 }
|}
  in
  Alcotest.(check int) "parses cleanly" 0 (List.length (errors_of ds));
  match file.Ast.decls with
  | [
   { Ast.dkind = Ast.DOp _; dtraits = [ { Ast.tname = "doc"; _ } ]; _ };
   { Ast.dkind = Ast.DStruct _; dtraits = []; _ };
  ] ->
      ()
  | _ -> Alcotest.fail "expected the op to own the trait"

(* The [fmt] pipeline behind the CLI: canonical output on success, joined
   diagnostics on parse errors. *)
let format_source_ok () =
  match format_source "struct point { x: i64, y: i64 }" with
  | Ok out ->
      Alcotest.(check string)
        "canonical form" "struct point {\n  x: i64\n  y: i64\n}\n" out
  | Error msg -> Alcotest.failf "expected Ok, got: %s" msg

let format_source_error () =
  match format_source "struct {" with
  | Ok _ -> Alcotest.fail "expected an error for malformed source"
  | Error msg -> Alcotest.(check bool) "non-empty message" true (msg <> "")

(* Imports (with and without an alias) and a qualified type reference print in the
   canonical layout: the import block first, then a blank line, then the
   declarations. This pins the module-syntax formatting deterministically,
   independent of the property test's random files. *)
let format_imports_and_qualified_refs () =
  let src =
    "import payments.common\n\
     import billing.tax as t\n\n\
     pub struct charge { total: common.money, vat: t.rate }\n"
  in
  let expected =
    "import payments.common\n\
     import billing.tax as t\n\n\
     pub struct charge {\n\
    \  total: common.money\n\
    \  vat: t.rate\n\
     }\n"
  in
  match format_source src with
  | Ok out -> Alcotest.(check string) "canonical layout" expected out
  | Error msg -> Alcotest.failf "expected Ok, got: %s" msg

(* An import-only file keeps just the import block (no trailing declaration
   separator). *)
let format_imports_only () =
  match format_source "import payments.common\n" with
  | Ok out ->
      Alcotest.(check string) "imports only" "import payments.common\n" out
  | Error msg -> Alcotest.failf "expected Ok, got: %s" msg

let () =
  Alcotest.run "fmt"
    [
      ( "printer",
        [
          Alcotest.test_case "golden layout" `Quick golden_layout;
          Alcotest.test_case "idempotent" `Quick idempotent;
          Alcotest.test_case "IR equivalent" `Quick ir_equivalent;
          Alcotest.test_case "literals round-trip" `Quick literals;
          Alcotest.test_case "defensive placeholders" `Quick
            defensive_placeholders;
          Alcotest.test_case "op swallows following traits" `Quick
            op_swallows_following_traits;
          Alcotest.test_case "ext layout" `Quick ext_layout;
          Alcotest.test_case "entry layout" `Quick entry_layout;
          Alcotest.test_case "entry IR equivalent" `Quick entry_ir_equivalent;
          Alcotest.test_case "ext binding escapes" `Quick ext_binding_escapes;
        ] );
      ( "format_source",
        [
          Alcotest.test_case "happy path" `Quick format_source_ok;
          Alcotest.test_case "parse errors abort" `Quick format_source_error;
          Alcotest.test_case "imports and qualified refs" `Quick
            format_imports_and_qualified_refs;
          Alcotest.test_case "imports only" `Quick format_imports_only;
        ] );
    ]
