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
@open enum currency { usd eur }
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

@open
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

op create_charge(charge): charge @errors(not_found, conflict) @http(method: "post")

op ping()
|}
  in
  Alcotest.(check string) "canonical layout" expected (fmt src)

(* Formatting is a fixpoint: formatting already-formatted output is identity. *)
let idempotent () =
  let src =
    {|
@doc("payments") pub struct charge { id: uuid, note: string? }
@open enum currency { usd, eur }
union source { card(card), bank(bank_account) }
op create_charge(charge): charge @errors(not_found)
|}
  in
  let once = fmt src in
  Alcotest.(check string) "fmt (fmt src) = fmt src" once (fmt once)

(* Formatting preserves meaning: the formatted source compiles to the same IR. *)
let ir_equivalent () =
  let src =
    {|
@doc("payments") pub struct charge {
  id: uuid
  amount_cents: i64 @range(min: 0)
  note: string?
}
@open
enum currency { usd, eur }
enum http_code { ok = 200, error = 500 }
@discriminator("kind")
union source { card(card), bank(bank_account) }
struct page[t] { items: []t, next: string? }
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
  match file with
  | [
   { Ast.dkind = Ast.DOp _; dtraits = [ { Ast.tname = "doc"; _ } ]; _ };
   { Ast.dkind = Ast.DStruct _; dtraits = []; _ };
  ] ->
      ()
  | _ -> Alcotest.fail "expected the op to own the trait"

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
        ] );
    ]
