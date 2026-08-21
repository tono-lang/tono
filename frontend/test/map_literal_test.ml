open Tono_frontend

(* A call: argument that is a map literal ([{ "key": value, ... }]): the
   key-value sibling of the list literal, the caller's own value for a
   [map[string]V] logical parameter. Parsed, printed back, lowered to the IR
   as ordered pairs, round-tripped through the JSON codec; a duplicate key is
   a parse error, and the nested shapes (a parameter reference, a nested
   call, a class reference) are walked inside the map the way they are
   inside a list. *)

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
  go: "tono-ext-fixture/mathkit"
  ts: "@tono-ext-fixture/mathkit"
  rust: "mathkit"

  type calculator("Calculator", float) interface {
    extern compute(): float {
      go { call: "Compute"() ctx }
      ts { call: "compute"() sync }
      rust { call: "compute"() }
    }
  }

  extern from_table(table: map[string]float): calculator {
    go { call: "FromTable"(table) }
    ts { call: "TableCalculator"(table) new }
    rust { call: "from_table"(table) }
  }

  extern scaled(factor: float): calculator {
    go { call: "Scaled"({ "factor": factor, "opts": "WithScale"(factor) }) }
  }
}

pub struct client {
  answer: float @arg

  table: mathkit.calculator = mathkit.from_table({ "answer": .answer, "other": 1.5 })
  empty: mathkit.calculator = mathkit.from_table({})

  op value(): float
    impl .table.compute()

  op nothing(): float
    impl .empty.compute()
}
|}

let contains haystack needle =
  let n = String.length needle in
  let rec find i =
    i + n <= String.length haystack
    && (String.sub haystack i n = needle || find (i + 1))
  in
  find 0

let map_literal_parses_and_lowers () =
  let file, diags = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length diags);
  Alcotest.(check bool) "typechecks" true (check src = []);
  let m = Lower.lower_file ~module_name:"m" ~diags:(ref []) file in
  let lib = List.hd m.Ir.ext_libs in
  let scaled =
    List.find
      (fun (e : Ir.extern_decl) -> e.x_name = "scaled")
      lib.Ir.xl_externs
  in
  (match (List.hd scaled.Ir.x_langs).el_call_args with
  | [
   Ir.Ca_map
     [
       ("factor", Ir.Ca_param "factor");
       ( "opts",
         Ir.Ca_symbol_call
           { Ir.scl_symbol = "WithScale"; scl_args = [ Ir.Ca_param "factor" ] }
       );
     ];
  ] ->
      ()
  | _ -> Alcotest.fail "the call: line's map literal did not lower as pairs");
  let fields =
    List.find_map
      (fun (s : Ir.shape) ->
        match s.Ir.kind with Ir.Entry { fields; _ } -> Some fields | _ -> None)
      m.Ir.shapes
    |> Option.get
  in
  let field name =
    List.find (fun (f : Ir.entry_field) -> f.Ir.ef_name = name) fields
  in
  (match (Option.get (field "table").Ir.ef_call).Ir.ec_args with
  | [ Ir.Ca_map [ ("answer", Ir.Ca_ref [ "answer" ]); ("other", Ir.Ca_lit _) ] ]
    ->
      ()
  | _ -> Alcotest.fail "the field's map literal did not lower in written order");
  match (Option.get (field "empty").Ir.ef_call).Ir.ec_args with
  | [ Ir.Ca_map [] ] -> ()
  | _ -> Alcotest.fail "the empty map literal did not lower"

let map_literal_prints_back () =
  let file, _ = Parser.parse src in
  let printed = Printer.print_file file in
  Alcotest.(check bool)
    "printed form keeps the map literal" true
    (contains printed {|{ "answer": .answer, "other": 1.5 }|});
  Alcotest.(check bool)
    "printed form keeps the empty map" true
    (contains printed "from_table({})");
  let reparsed, diags = Parser.parse printed in
  Alcotest.(check int) "no reparse diagnostics" 0 (List.length diags);
  Alcotest.(check string) "idempotent" printed (Printer.print_file reparsed)

let map_literal_roundtrips_json () =
  let file, _ = Parser.parse src in
  let m = Lower.lower_file ~module_name:"m" ~diags:(ref []) file in
  let full : Ir.model =
    { tono_ir_version = Ir_json.current_ir_version; modules = [ m ] }
  in
  let json = Ir_json.encode_model full in
  Alcotest.(check bool)
    "encoded as ordered pairs" true
    (contains
       (Ir_json.to_canonical_string json)
       {|{"map":[["answer",{"field":["answer"]}],["other",{"lit":1.5}]]}|});
  match Ir_json.decode_model json with
  | Error e -> Alcotest.failf "decode failed: %s" e
  | Ok decoded ->
      Alcotest.(check string)
        "round-trip"
        (Ir_json.to_canonical_string json)
        (Ir_json.to_canonical_string (Ir_json.encode_model decoded))

let malformed_map_entry_rejected () =
  let file, _ = Parser.parse src in
  let m = Lower.lower_file ~module_name:"m" ~diags:(ref []) file in
  let full : Ir.model =
    { tono_ir_version = Ir_json.current_ir_version; modules = [ m ] }
  in
  let text = Ir_json.to_canonical_string (Ir_json.encode_model full) in
  let good = {|["answer",{"field":["answer"]}]|} in
  let bad = {|[1,{"lit":1}]|} in
  let i =
    let rec find i =
      if String.sub text i (String.length good) = good then i else find (i + 1)
    in
    find 0
  in
  let broken =
    String.sub text 0 i ^ bad
    ^ String.sub text
        (i + String.length good)
        (String.length text - i - String.length good)
  in
  match Ir_json.decode_model (Yojson.Safe.from_string broken) with
  | Error e ->
      Alcotest.(check bool)
        "names the pair shape" true
        (contains e "[key, value] pair")
  | Ok _ -> Alcotest.fail "a non-string key decoded"

let duplicate_key_rejected () =
  let src =
    {|ext mathkit {
  go: "tono-ext-fixture/mathkit"

  extern scaled(factor: float): float {
    go { call: "Scaled"({ "factor": factor, "factor": 2 }) }
  }
}
|}
  in
  let _, diags = Parser.parse src in
  Alcotest.(check bool)
    "duplicate key diagnosed" true
    (List.exists
       (fun (d : Diagnostic.t) -> contains d.Diagnostic.message "duplicate key")
       diags)

let non_string_key_rejected () =
  let src =
    {|ext mathkit {
  go: "tono-ext-fixture/mathkit"

  extern scaled(factor: float): float {
    go { call: "Scaled"({ factor: 2 }) }
  }
}
|}
  in
  let _, diags = Parser.parse src in
  Alcotest.(check bool) "non-string key diagnosed" true (diags <> [])

(* The per-item checks a list already gets apply inside a map too: an
   undeclared parameter (TC0082-family), an unknown class reference
   (TC0098), a class reference outside a binding. *)
let unknown_param_inside_a_map_rejected () =
  let src =
    {|ext mathkit {
  go: "tono-ext-fixture/mathkit"

  extern scaled(factor: float): float {
    go { call: "Scaled"({ "factor": missing }) }
  }
}
|}
  in
  Alcotest.(check bool)
    "diagnosed" true
    (has Error_codes.extern_call_unknown_param src)

let unknown_handle_inside_a_map_rejected () =
  let src =
    {|ext mathkit {
  ts: "@tono-ext-fixture/mathkit"

  type calculator {
    extern compute(): float {
      ts { call: "compute"() sync }
    }
  }

  extern make(): calculator {
    ts { call: "instantiate"({ "impl": type missing }) sync }
  }
}
|}
  in
  Alcotest.(check bool) "TC0098" true (has "TC0098" src)

let unknown_field_inside_a_map_rejected () =
  let src =
    {|ext mathkit {
  go: "tono-ext-fixture/mathkit"

  type calculator {
    extern compute(): float {
      go { call: "Compute"() ctx }
    }
  }

  extern from_table(table: map[string]float): calculator {
    go { call: "FromTable"(table) }
  }
}

pub struct client {
  table: mathkit.calculator = mathkit.from_table({ "answer": .missing })

  op value(): float
    impl .table.compute()
}
|}
  in
  Alcotest.(check bool) "diagnosed" true (check src <> [])

let () =
  Alcotest.run "map_literal"
    [
      ( "surface",
        [
          Alcotest.test_case "parses and lowers" `Quick
            map_literal_parses_and_lowers;
          Alcotest.test_case "prints back" `Quick map_literal_prints_back;
          Alcotest.test_case "round-trips the IR" `Quick
            map_literal_roundtrips_json;
          Alcotest.test_case "malformed entry rejected" `Quick
            malformed_map_entry_rejected;
        ] );
      ( "diagnostics",
        [
          Alcotest.test_case "duplicate key rejected" `Quick
            duplicate_key_rejected;
          Alcotest.test_case "non-string key rejected" `Quick
            non_string_key_rejected;
          Alcotest.test_case "unknown param inside a map rejected" `Quick
            unknown_param_inside_a_map_rejected;
          Alcotest.test_case "unknown handle inside a map rejected" `Quick
            unknown_handle_inside_a_map_rejected;
          Alcotest.test_case "unknown field inside a map rejected" `Quick
            unknown_field_inside_a_map_rejected;
        ] );
    ]
