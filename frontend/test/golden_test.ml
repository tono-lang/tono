open Tono_frontend

let canon j = Ir_json.to_canonical_string j
let module_json m = canon (Ir_json.encode_module m)

(* A small file compiled end to end, checked against an IR module built by hand.
   Comparing canonical JSON keeps the assertion stable across key ordering. *)
let golden_module () =
  let src =
    {|
struct point { x: i64, y: i64 }
enum dir { north, south }
op origin(): point
|}
  in
  let compiled, ds = Tono_frontend.compile ~module_name:"geo" src in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  let i64 = Ir.Prim (Ir.int_prim ~bits:64 ~signed:true) in
  let member name target : Ir.member =
    {
      name;
      target;
      required = true;
      default = None;
      constraints = [];
      traits = [];
    }
  in
  let expected : Ir.module_ =
    {
      mod_name = "geo";
      shapes =
        [
          {
            id = "geo#point";
            kind =
              Ir.Structure
                { params = []; members = [ member "x" i64; member "y" i64 ] };
            traits = [];
          };
          {
            id = "geo#dir";
            kind =
              Ir.Enum
                {
                  backing = `String;
                  values = [ Ir.enum_value "north"; Ir.enum_value "south" ];
                };
            traits = [];
          };
        ];
      operations =
        [
          {
            id = "geo#origin";
            kind =
              Ir.Operation
                {
                  input = None;
                  output = Some (Ir.Ref ("geo#point", []));
                  errors = [];
                };
            traits = [];
          };
        ];
      extensions = [];
    }
  in
  Alcotest.(check string)
    "module matches" (module_json expected) (module_json compiled)

(* A feature-rich file compiles cleanly and the resulting IR survives a JSON
   round-trip (encode, decode, re-encode), exercising the whole pipeline against
   the wire contract the backend mirrors. *)
let rich_roundtrip () =
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

op create_charge(charge): charge @errors(not_found) @async
|}
  in
  let m, ds = Tono_frontend.compile ~module_name:"payments" src in
  Alcotest.(check int) "compiles cleanly" 0 (List.length ds);
  Alcotest.(check (list string))
    "shape ids"
    [
      "payments#charge";
      "payments#currency";
      "payments#http_code";
      "payments#card";
      "payments#bank_account";
      "payments#source";
      "payments#page";
      "payments#not_found";
    ]
    (List.map (fun (s : Ir.shape) -> s.id) m.shapes);
  Alcotest.(check (list string))
    "operation ids"
    [ "payments#create_charge" ]
    (List.map (fun (s : Ir.shape) -> s.id) m.operations);
  let json = Ir_json.encode_module m in
  match Ir_json.decode_module json with
  | Error e -> Alcotest.failf "module did not round-trip: %s" e
  | Ok m' ->
      Alcotest.(check string)
        "re-encode is identical" (canon json) (module_json m')

(* Every extension kind compiles into the module's extension table and the
   IR survives a JSON round-trip, exercising the ext surface end to end. *)
let extension_roundtrip () =
  let src =
    {|
ext hook before_request {
  ts: "ext/ts/auth.ts#addBearer"
  rust: "ext/rust/auth.rs#add_bearer"
}

ext contract sign_request (canonical_request) -> string {
  ts: "ext/ts/sign.ts#signRequest"
  conformance: "vectors/sign_request.json"
}

ext constraint luhn (string) -> bool {
  ts: "ext/ts/luhn.ts#isLuhn"
}

ext impl save_note raw {
  go: "ext/go/save.go#SaveNote"
  ts: "ext/ts/save.ts#saveNote"
  conformance: "vectors/save_note.json"
}

struct note {
  id: string
}

pub struct client {
  ep: string @env("EP")

  op save_note(note): note
}
|}
  in
  let m, ds = Tono_frontend.compile ~module_name:"payments" src in
  Alcotest.(check int) "compiles cleanly" 0 (List.length ds);
  Alcotest.(check (list string))
    "extension names"
    [ "before_request"; "sign_request"; "luhn"; "save_note" ]
    (List.map (fun (e : Ir.extension) -> e.ext_name) m.extensions);
  let json = Ir_json.encode_module m in
  match Ir_json.decode_module json with
  | Error e -> Alcotest.failf "module did not round-trip: %s" e
  | Ok m' ->
      Alcotest.(check string)
        "re-encode is identical" (canon json) (module_json m')

let () =
  Alcotest.run "golden"
    [
      ( "end-to-end",
        [
          Alcotest.test_case "golden module" `Quick golden_module;
          Alcotest.test_case "rich round-trip" `Quick rich_roundtrip;
          Alcotest.test_case "extension round-trip" `Quick extension_roundtrip;
        ] );
    ]
