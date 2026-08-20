open Tono_frontend

(* A call: line whose receiver is a foreign type name ("Type"."method"): a
   static method of the library, qualified by the type rather than a value.
   Accepted on a free extern, printed back the same way, carried into the
   IR as the binding's receiver; rejected on a handle's own method (TC0096)
   and together with the "new" marker (TC0097). *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let has code src =
  List.mem code (List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src))

let src =
  {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"
  rust: "mathkit"

  type calculator("Calculator", float) {
    extern compute(): float {
      ts { call: "compute"() sync }
      rust { call: "compute"() }
    }
  }

  extern parse(expr: string): calculator {
    ts { call: "FormulaCalculator"."parse"(expr) sync }
    rust { call: "FormulaCalculator"."parse"(expr) sync }
  }
}

pub struct client {
  expr: string @arg
  calc: mathkit.calculator = mathkit.parse(.expr)

  op value(): float
    impl .calc.compute()
}
|}

let receiver_parses_and_lowers () =
  let file, diags = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length diags);
  Alcotest.(check bool) "typechecks" true (check src = []);
  let lower_diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags:lower_diags file in
  let lib = List.hd m.Ir.ext_libs in
  let parse =
    List.find (fun (e : Ir.extern_decl) -> e.x_name = "parse") lib.Ir.xl_externs
  in
  List.iter
    (fun (l : Ir.extern_lang) ->
      Alcotest.(check (option string))
        ("receiver in " ^ l.el_lang)
        (Some "FormulaCalculator") l.el_receiver;
      Alcotest.(check string) "method" "parse" l.el_symbol)
    parse.Ir.x_langs;
  let compute = List.hd (List.hd lib.Ir.xl_types).Ir.opq_methods in
  List.iter
    (fun (l : Ir.extern_lang) ->
      Alcotest.(check (option string))
        "no receiver on a method" None l.el_receiver)
    compute.Ir.x_langs

let receiver_prints_back () =
  let file, _ = Parser.parse src in
  let printed = Printer.print_file file in
  Alcotest.(check bool)
    "printed form keeps the receiver" true
    (let needle = {|call: "FormulaCalculator"."parse"(expr)|} in
     let n = String.length needle in
     let rec find i =
       i + n <= String.length printed
       && (String.sub printed i n = needle || find (i + 1))
     in
     find 0);
  let reparsed, diags = Parser.parse printed in
  Alcotest.(check int) "no reparse diagnostics" 0 (List.length diags);
  Alcotest.(check string) "idempotent" printed (Printer.print_file reparsed)

let receiver_roundtrips_json () =
  let file, _ = Parser.parse src in
  let m = Lower.lower_file ~module_name:"m" ~diags:(ref []) file in
  let full : Ir.model =
    { tono_ir_version = Ir_json.current_ir_version; modules = [ m ] }
  in
  let json = Ir_json.encode_model full in
  match Ir_json.decode_model json with
  | Error e -> Alcotest.failf "decode failed: %s" e
  | Ok decoded ->
      Alcotest.(check string)
        "round-trip"
        (Ir_json.to_canonical_string json)
        (Ir_json.to_canonical_string (Ir_json.encode_model decoded))

let receiver_on_handle_method_rejected () =
  let src =
    {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"

  type calculator {
    extern compute(): float {
      ts { call: "Calculator"."compute"() sync }
    }
  }

  extern parse(expr: string): calculator {
    ts { call: "FormulaCalculator"."parse"(expr) sync }
  }
}
|}
  in
  Alcotest.(check bool) "TC0096" true (has "TC0096" src)

let receiver_with_new_rejected () =
  let src =
    {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"

  type calculator {
    extern compute(): float {
      ts { call: "compute"() sync }
    }
  }

  extern parse(expr: string): calculator {
    ts { call: "FormulaCalculator"."parse"(expr) new }
  }
}
|}
  in
  Alcotest.(check bool) "TC0097" true (has "TC0097" src)

let missing_method_string_diagnosed () =
  let src =
    {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"

  extern parse(expr: string): string {
    ts { call: "FormulaCalculator".(expr) }
  }
}
|}
  in
  let _, diags = Parser.parse src in
  Alcotest.(check bool) "a parse diagnostic" true (diags <> [])

let () =
  Alcotest.run "static_receiver"
    [
      ( "surface",
        [
          Alcotest.test_case "parses and lowers" `Quick
            receiver_parses_and_lowers;
          Alcotest.test_case "prints back" `Quick receiver_prints_back;
          Alcotest.test_case "round-trips the IR" `Quick
            receiver_roundtrips_json;
          Alcotest.test_case "dot without a method string" `Quick
            missing_method_string_diagnosed;
        ] );
      ( "scope",
        [
          Alcotest.test_case "receiver on a handle method rejected" `Quick
            receiver_on_handle_method_rejected;
          Alcotest.test_case "receiver with new rejected" `Quick
            receiver_with_new_rejected;
        ] );
    ]
