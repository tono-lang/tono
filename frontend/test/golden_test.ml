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
op origin() -> point
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
            id = "point";
            kind =
              Ir.Structure
                { params = []; members = [ member "x" i64; member "y" i64 ] };
            traits = [];
          };
          {
            id = "dir";
            kind =
              Ir.Enum
                {
                  backing = `String;
                  values = [ ("north", None); ("south", None) ];
                  open_ = false;
                };
            traits = [];
          };
        ];
      operations =
        [
          {
            id = "origin";
            kind =
              Ir.Operation
                {
                  input = None;
                  output = Some (Ir.Ref ("point", []));
                  errors = [];
                };
            traits = [];
          };
        ];
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

@open
enum currency { usd, eur }

enum http_code { ok = 200, error = 500 }

@discriminator("kind")
union source { card: card, bank: bank_account }

struct page[t] { items: []t, next: string? }

op create_charge(charge) -> charge throws not_found
|}
  in
  let m, ds = Tono_frontend.compile ~module_name:"payments" src in
  Alcotest.(check int) "compiles cleanly" 0 (List.length ds);
  Alcotest.(check (list string))
    "shape ids"
    [ "charge"; "currency"; "http_code"; "source"; "page" ]
    (List.map (fun (s : Ir.shape) -> s.id) m.shapes);
  Alcotest.(check (list string))
    "operation ids" [ "create_charge" ]
    (List.map (fun (s : Ir.shape) -> s.id) m.operations);
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
        ] );
    ]
