open Tono_frontend

(* A foreign spelling, #(...): lexed as one raw token up to the matching
   parenthesis, printed back byte for byte, and read in every position of
   a call: line: the callee (a function, a class under new, a static method
   on a type), a parameter crossing under its own spelling, a nested call,
   and a declared position the target binds itself. A bare name that is an
   opaque handle of the block or a wire struct of the module (and no
   parameter) is a class reference; one that is both is ambiguous
   (TC0098). *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let has code src =
  List.mem code (List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src))

let contains haystack needle =
  let n = String.length needle in
  let rec find i =
    i + n <= String.length haystack
    && (String.sub haystack i n = needle || find (i + 1))
  in
  find 0

(* ── lexer ─────────────────────────────────────────────────────────────── *)

let foreign_of src =
  List.filter_map
    (fun (t : Token.t) ->
      match t.kind with Token.Foreign s -> Some s | _ -> None)
    (fst (Lexer.tokenize src))

let nested_parentheses_lex_as_one_token () =
  Alcotest.(check (list string))
    "balanced pairs stay inside"
    [ "Error()"; "func(int) error"; "Vec<f64>" ]
    (foreign_of "#(Error()) #(func(int) error) #(Vec<f64>)")

let spelling_is_never_decoded () =
  Alcotest.(check (list string))
    "quotes and escapes are bytes" [ "a \"b\" \\n" ]
    (foreign_of "#(a \"b\" \\n)")

let unbalanced_spelling_is_refused () =
  let _, diags = Lexer.tokenize "#(Error(" in
  Alcotest.(check bool)
    "unterminated" true
    (List.exists
       (fun (d : Diagnostic.t) -> contains d.message "no matching ')'")
       diags);
  let _, diags = Lexer.tokenize "#x" in
  Alcotest.(check bool)
    "bare hash" true
    (List.exists
       (fun (d : Diagnostic.t) -> contains d.message "expected '('")
       diags)

(* ── call: positions ───────────────────────────────────────────────────── *)

let src =
  {|ext mathkit {
  go { #(tono-ext-fixture/mathkit) }
  ts { #(@tono-ext-fixture/mathkit) }
  rust { #(mathkit) }

  struct calculator {
    go { #(Calculator[float64]) }
    ts { #(Calculator<number>) }
    rust { #(Box<dyn Calculator<f64>>) }

    @async(rust)
    op compute(): float {
      go { call: #(Compute)(#(ctx context.Context)) }
      ts { call: #(compute)() }
      rust { call: #(compute)() }
    }
  }

  struct answer_calculator {
    ts { #(AnswerCalculator) }

    op compute(): float {
      ts { call: #(compute)() }
    }
  }

  struct options {
    addr: string
    go { #(Options) }
  }

  op from_constant(value: float): calculator {
    go { call: #(FromConstant[float64])(value) }
    ts { call: #(new ConstantCalculator)(value) }
    rust { call: #(from_constant)(value) }
  }

  op from_series(values: []float): calculator {
    go { call: #(FromSeries[float64])(values: #([]float64)) }
    ts { call: #(new SeriesCalculator)(values: #(number[])) }
    rust { call: #(from_series)(values: #(Vec<f64>)) }
  }

  op from_formula(expr: string, precision: u8): calculator {
    go { call: #(FromFormula[float64])(expr, #(WithPrecision)(precision)) }
    ts { call: #(FormulaCalculator.parse)(expr, precision) }
    rust { call: #(FormulaCalculator::parse)(expr, precision) }
  }

  op instantiate(): calculator {
    ts { call: #(instantiate)(answer_calculator) }
  }

  op connect(addr: string): calculator {
    go { call: #(Connect[float64])(options { addr: addr }: #(&Options)) }
  }
}

pub struct client {
  base: float @arg
  constant: mathkit.calculator = mathkit.from_constant(.base)

  op value(): float
    impl .constant.compute()
}
|}

let lowered () =
  let file, diags = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length diags);
  Alcotest.(check (list string))
    "typechecks" []
    (List.map (fun (d : Diagnostic.t) -> d.message) (check src));
  let m = Lower.lower_file ~module_name:"m" ~diags:(ref []) file in
  List.hd m.Ir.ext_libs

let extern_named lib name =
  List.find (fun (e : Ir.extern_decl) -> e.Ir.x_name = name) lib.Ir.xl_externs

let lang (e : Ir.extern_decl) l =
  List.find (fun (b : Ir.extern_lang) -> b.Ir.el_lang = l) e.Ir.x_langs

let callee_is_the_whole_spelling () =
  let lib = lowered () in
  let fc = extern_named lib "from_constant" in
  Alcotest.(check string)
    "generic instantiation" "FromConstant[float64]" (lang fc "go").el_symbol;
  Alcotest.(check string)
    "class under new" "new ConstantCalculator" (lang fc "ts").el_symbol;
  let ff = extern_named lib "from_formula" in
  Alcotest.(check string)
    "static method" "FormulaCalculator::parse" (lang ff "rust").el_symbol

let parameter_under_its_own_spelling () =
  let lib = lowered () in
  let fs = extern_named lib "from_series" in
  Alcotest.(check bool)
    "param_as in rust" true
    ((lang fs "rust").el_call_args = [ Ir.Ca_param_as ("values", "Vec<f64>") ]);
  Alcotest.(check bool)
    "param_as in go" true
    ((lang fs "go").el_call_args = [ Ir.Ca_param_as ("values", "[]float64") ])

(* The same annotation on a struct literal: the form's type stays the type
   the block declares (#(Options)); the spelling on the argument says how
   the literal crosses (a pointer to it). *)
let struct_literal_under_its_own_spelling () =
  let lib = lowered () in
  let connect = extern_named lib "connect" in
  Alcotest.(check bool)
    "ctor carries its spelling" true
    ((lang connect "go").el_call_args
    = [
        Ir.Ca_ctor
          {
            Ir.cc_name = "options";
            cc_fields = [ ("addr", Ir.Ca_param "addr") ];
            cc_as = Some "&Options";
          };
      ]);
  let form =
    List.find
      (fun (s : Ir.foreign_struct) -> s.Ir.fgs_name = "options")
      lib.Ir.xl_structs
  in
  Alcotest.(check bool)
    "the form's own type has no &" true
    (List.exists
       (fun (b : Ir.foreign_lang) ->
         b.Ir.fl_lang = "go" && b.Ir.fl_head = "Options")
       form.Ir.fgs_langs)

let nested_call_and_bound_position () =
  let lib = lowered () in
  let ff = extern_named lib "from_formula" in
  Alcotest.(check bool)
    "nested call keeps its spelling" true
    ((lang ff "go").el_call_args
    = [
        Ir.Ca_param "expr";
        Ir.Ca_symbol_call
          {
            Ir.scl_symbol = "WithPrecision";
            scl_args = [ Ir.Ca_param "precision" ];
          };
      ]);
  let compute = List.hd (List.hd lib.Ir.xl_types).Ir.opq_methods in
  Alcotest.(check bool)
    "ctx is a declared position" true
    ((lang compute "go").el_call_args = [ Ir.Ca_foreign "ctx context.Context" ]);
  Alcotest.(check (list string))
    "async lists rust" [ "rust" ] compute.Ir.x_async

let handle_name_is_a_class_reference () =
  let lib = lowered () in
  let inst = extern_named lib "instantiate" in
  Alcotest.(check bool)
    "class reference" true
    ((lang inst "ts").el_call_args = [ Ir.Ca_type "answer_calculator" ])

let prints_back_and_roundtrips () =
  let file, _ = Parser.parse src in
  let printed = Printer.print_file file in
  List.iter
    (fun needle ->
      Alcotest.(check bool)
        ("printed keeps " ^ needle)
        true (contains printed needle))
    [
      "call: #(new ConstantCalculator)(value)";
      "call: #(FormulaCalculator::parse)(expr, precision)";
      "values: #(Vec<f64>)";
      "options { addr: addr }: #(&Options)";
      "#(ctx context.Context)";
      "#(WithPrecision)(precision)";
      "rust { #(Box<dyn Calculator<f64>>) }";
      "@async(rust)";
    ];
  let reparsed, diags = Parser.parse printed in
  Alcotest.(check int) "no reparse diagnostics" 0 (List.length diags);
  Alcotest.(check string) "idempotent" printed (Printer.print_file reparsed);
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

(* ── refusals ──────────────────────────────────────────────────────────── *)

let with_call call =
  Printf.sprintf
    {|ext mathkit {
  ts { #(@tono-ext-fixture/mathkit) }

  struct calculator {
    ts { #(Calculator<number>) }
  }

  struct answer { ts { #(Answer) } }

  op make(answer: string, seed: string): calculator {
    ts { call: %s }
  }
}
|}
    call

let parameter_and_handle_collide () =
  Alcotest.(check bool)
    "TC0098" true
    (has "TC0098" (with_call "#(instantiate)(answer, seed)"))

let unknown_bare_name_rejected () =
  let src = with_call "#(instantiate)(seed, other)" in
  Alcotest.(check bool) "TC0070" true (has "TC0070" src)

(* The ext above beside the module's own shapes: a wire struct (profile),
   a generic one (box), and a config (cfg). *)
let with_module ?(params = "answer: string, seed: string") call =
  Printf.sprintf
    {|ext mathkit {
  ts { #(@tono-ext-fixture/mathkit) }

  struct calculator {
    ts { #(Calculator<number>) }
  }

  op make(%s): calculator {
    ts { call: %s }
  }
}

struct profile {
  endpoint: string
}

struct box[t] {
  value: t
}

struct cfg {
  host: string @env(HOST)
}
|}
    params call

let struct_name_is_a_class_reference () =
  let src = with_module "#(instantiate)(answer, seed, profile)" in
  Alcotest.(check bool) "not TC0070" false (has "TC0070" src);
  let file, _ = Parser.parse src in
  let m = Lower.lower_file ~module_name:"m" ~diags:(ref []) file in
  let make = extern_named (List.hd m.Ir.ext_libs) "make" in
  Alcotest.(check bool)
    "the struct lowers as the class, the parameters as themselves" true
    ((lang make "ts").el_call_args
    = [ Ir.Ca_param "answer"; Ir.Ca_param "seed"; Ir.Ca_type "profile" ])

let only_a_wire_struct_is_a_class_reference () =
  Alcotest.(check bool)
    "generic: TC0070" true
    (has "TC0070" (with_module "#(instantiate)(answer, seed, box)"));
  Alcotest.(check bool)
    "config: TC0070" true
    (has "TC0070" (with_module "#(instantiate)(answer, seed, cfg)"))

let parameter_and_struct_collide () =
  let src =
    with_module ~params:"profile: string, seed: string"
      "#(instantiate)(profile, seed)"
  in
  Alcotest.(check bool) "TC0098" true (has "TC0098" src)

let spelling_required_after_parameter_colon () =
  let _, diags = Parser.parse (with_call "#(instantiate)(answer: string)") in
  Alcotest.(check bool)
    "a parse diagnostic" true
    (List.exists
       (fun (d : Diagnostic.t) -> contains d.message "foreign spelling")
       diags)

let spelling_required_after_literal_colon () =
  let _, diags = Parser.parse (with_call "#(instantiate)(answer { }: seed)") in
  Alcotest.(check bool)
    "a parse diagnostic" true
    (List.exists
       (fun (d : Diagnostic.t) ->
         contains d.message "foreign spelling"
         && contains d.message "struct literal")
       diags)

let callee_must_be_a_spelling () =
  let _, diags = Parser.parse (with_call "\"instantiate\"(answer)") in
  Alcotest.(check bool)
    "a string is not a callee" true
    (List.exists
       (fun (d : Diagnostic.t) -> contains d.message "callee after 'call:'")
       diags)

let () =
  Alcotest.run "foreign_spelling"
    [
      ( "lexer",
        [
          Alcotest.test_case "nested parentheses" `Quick
            nested_parentheses_lex_as_one_token;
          Alcotest.test_case "never decoded" `Quick spelling_is_never_decoded;
          Alcotest.test_case "unbalanced refused" `Quick
            unbalanced_spelling_is_refused;
        ] );
      ( "call positions",
        [
          Alcotest.test_case "callee is the whole spelling" `Quick
            callee_is_the_whole_spelling;
          Alcotest.test_case "parameter under its own spelling" `Quick
            parameter_under_its_own_spelling;
          Alcotest.test_case "struct literal under its own spelling" `Quick
            struct_literal_under_its_own_spelling;
          Alcotest.test_case "nested call and bound position" `Quick
            nested_call_and_bound_position;
          Alcotest.test_case "handle name is a class reference" `Quick
            handle_name_is_a_class_reference;
          Alcotest.test_case "struct name is a class reference" `Quick
            struct_name_is_a_class_reference;
          Alcotest.test_case "prints back and round-trips" `Quick
            prints_back_and_roundtrips;
        ] );
      ( "refusals",
        [
          Alcotest.test_case "parameter and handle collide" `Quick
            parameter_and_handle_collide;
          Alcotest.test_case "unknown bare name" `Quick
            unknown_bare_name_rejected;
          Alcotest.test_case "only a wire struct is a class" `Quick
            only_a_wire_struct_is_a_class_reference;
          Alcotest.test_case "parameter and struct collide" `Quick
            parameter_and_struct_collide;
          Alcotest.test_case "spelling after colon" `Quick
            spelling_required_after_parameter_colon;
          Alcotest.test_case "spelling after a literal's colon" `Quick
            spelling_required_after_literal_colon;
          Alcotest.test_case "callee must be a spelling" `Quick
            callee_must_be_a_spelling;
        ] );
    ]
