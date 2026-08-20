open Tono_frontend

(* A call: argument passing a class reference ([type handle]): a declared
   opaque handle of the same ext block, passed as a value for a library that
   takes the class itself and constructs on its own. Parsed, printed back,
   lowered to the IR as the handle's name; rejected when the name is not a
   handle of the block (TC0098) and outside a language block's call: line. *)

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

  type answer_calculator {
    extern compute(): float {
      ts { call: "compute"() sync }
      rust { call: "compute"() }
    }
  }

  extern make(): calculator {
    ts { call: "instantiate"(type answer_calculator) sync }
    rust { call: "instantiate"(type answer_calculator) sync }
  }
}

pub struct client {
  calc: mathkit.calculator = mathkit.make()

  op value(): float
    impl .calc.compute()
}
|}

let class_reference_parses_and_lowers () =
  let file, diags = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length diags);
  Alcotest.(check bool) "typechecks" true (check src = []);
  let m = Lower.lower_file ~module_name:"m" ~diags:(ref []) file in
  let lib = List.hd m.Ir.ext_libs in
  let make =
    List.find (fun (e : Ir.extern_decl) -> e.x_name = "make") lib.Ir.xl_externs
  in
  List.iter
    (fun (l : Ir.extern_lang) ->
      match l.el_call_args with
      | [ Ir.Ca_type "answer_calculator" ] -> ()
      | _ -> Alcotest.failf "unexpected call args in %s" l.el_lang)
    make.Ir.x_langs

let class_reference_prints_back () =
  let file, _ = Parser.parse src in
  let printed = Printer.print_file file in
  let contains needle =
    let n = String.length needle in
    let rec find i =
      i + n <= String.length printed
      && (String.sub printed i n = needle || find (i + 1))
    in
    find 0
  in
  Alcotest.(check bool)
    "printed form keeps the class reference" true
    (contains {|call: "instantiate"(type answer_calculator)|});
  let reparsed, diags = Parser.parse printed in
  Alcotest.(check int) "no reparse diagnostics" 0 (List.length diags);
  Alcotest.(check string) "idempotent" printed (Printer.print_file reparsed)

let class_reference_roundtrips_json () =
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

let unknown_handle_rejected () =
  let src =
    {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"

  type calculator {
    extern compute(): float {
      ts { call: "compute"() sync }
    }
  }

  extern make(): calculator {
    ts { call: "instantiate"(type missing) sync }
  }
}
|}
  in
  Alcotest.(check bool) "TC0098" true (has "TC0098" src)

let unknown_handle_inside_a_list_rejected () =
  let src =
    {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"

  type calculator {
    extern compute(): float {
      ts { call: "compute"() sync }
    }
  }

  extern make(): calculator {
    ts { call: "instantiate"(["Pick"(type missing)]) sync }
  }
}
|}
  in
  Alcotest.(check bool) "TC0098" true (has "TC0098" src)

(* "type" is contextual: a logical parameter spelled [type] is still a
   parameter when no identifier follows it. *)
let type_as_a_parameter_name_stays_a_parameter () =
  let src =
    {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"

  extern make(type: string): string {
    ts { call: "instantiate"(type) sync }
  }
}
|}
  in
  let file, diags = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length diags);
  Alcotest.(check bool) "typechecks" true (check src = []);
  let m = Lower.lower_file ~module_name:"m" ~diags:(ref []) file in
  let make = List.hd (List.hd m.Ir.ext_libs).Ir.xl_externs in
  match (List.hd make.Ir.x_langs).el_call_args with
  | [ Ir.Ca_param "type" ] -> ()
  | _ -> Alcotest.fail "expected a parameter named type"

(* An op's own handle-method call is a tono-level site passing values; a
   class reference only means something inside a language block. *)
let class_reference_outside_a_binding_rejected () =
  let src =
    {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"

  type calculator {
    extern compute(): float {
      ts { call: "compute"() sync }
    }
  }

  extern make(): calculator {
    ts { call: "instantiate"() sync }
  }
}

pub struct client {
  calc: mathkit.calculator = mathkit.make()

  op value(): float
    impl .calc.compute(type calculator)
}
|}
  in
  Alcotest.(check bool) "diagnosed" true (check src <> [])

let () =
  Alcotest.run "class_reference"
    [
      ( "surface",
        [
          Alcotest.test_case "parses and lowers" `Quick
            class_reference_parses_and_lowers;
          Alcotest.test_case "prints back" `Quick class_reference_prints_back;
          Alcotest.test_case "round-trips the IR" `Quick
            class_reference_roundtrips_json;
          Alcotest.test_case "type as a parameter name" `Quick
            type_as_a_parameter_name_stays_a_parameter;
        ] );
      ( "scope",
        [
          Alcotest.test_case "unknown handle rejected" `Quick
            unknown_handle_rejected;
          Alcotest.test_case "unknown handle inside a list rejected" `Quick
            unknown_handle_inside_a_list_rejected;
          Alcotest.test_case "outside a binding rejected" `Quick
            class_reference_outside_a_binding_rejected;
        ] );
    ]
