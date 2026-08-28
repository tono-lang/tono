open Tono_frontend

(* Language blocks on structs: an opaque handle's whole storage type per
   target, a foreign form's type and field spellings, and an error struct's
   sentinel and field sources, each lowered to the IR as written. With the
   rules that keep them honest: a block fits its struct (TC0092, TC0095,
   TC0097), a target the ext declares no module path for is named (TC0081),
   @async names only a target with an asynchronous call (TC0093), @errors
   resolves (TC0077), and no other trait applies to an ext op (TC0096). A
   wire struct's block has no head and declares the target's per-field
   tags (TC0100 for a head where none belongs, none where one is required,
   or no entry at all). *)

let parse_and_check src =
  let file, parse_diags = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  (* The module-path accounting is cross-file (decision K), so it runs as
     the project pass does for a lone module. *)
  let project = Check_ext_lib_project.check_project [ ("", file.Ast.decls) ] in
  (file, m, parse_diags, tc @ project)

let has code src =
  let _, _, _, tc = parse_and_check src in
  List.mem code (List.filter_map (fun (d : Diagnostic.t) -> d.code) tc)

let src =
  {|ext mathkit {
  go { #(tono-ext-fixture/mathkit) }
  rust { #(mathkit) }

  struct formula_options {
    precision: u8

    rust { #(FormulaOptions)  precision: #(Option<u8>) }
  }

  struct calculator {
    go { #(Calculator[float64]) }
    rust { #(Box<dyn Calculator<f64>>) }

    @async(rust)
    op compute(): float {
      go { call: #(Compute)(#(ctx context.Context)) }
      rust { call: #(compute)() }
    }
  }

  @errors(invalid_expression)
  op from_formula(expr: string, precision: u8): calculator {
    go { call: #(FromFormula[float64])(expr, #(WithPrecision)(precision)) }
    rust { call: #(from_formula)(expr, formula_options { precision: precision }) }
  }
}

@errorCode("code", "invalid_expression")
pub struct invalid_expression {
  message: string

  go { #(ErrParse)  message: #(Error()) }
  rust { #(Error::Parse)  message: #(to_string()) }
}

pub struct calibration {
  scale: float
  offset: float

  go { scale: #(env:"CALC_{profile}_SCALE") }
}
|}

let clean () =
  let file, m, parse_diags, tc = parse_and_check src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length parse_diags);
  Alcotest.(check (list string))
    "typechecks" []
    (List.map (fun (d : Diagnostic.t) -> d.message) tc);
  (file, m)

let handle_storage_type_per_target () =
  let _, m = clean () in
  let calc = List.hd (List.hd m.Ir.ext_libs).Ir.xl_types in
  Alcotest.(check bool)
    "whole storage type, per language" true
    (calc.Ir.opq_langs
    = [
        {
          Ir.fl_lang = "go";
          fl_head = Some "Calculator[float64]";
          fl_fields = [];
        };
        {
          Ir.fl_lang = "rust";
          fl_head = Some "Box<dyn Calculator<f64>>";
          fl_fields = [];
        };
      ])

let foreign_form_type_and_field_spelling () =
  let _, m = clean () in
  let opts = List.hd (List.hd m.Ir.ext_libs).Ir.xl_structs in
  Alcotest.(check bool)
    "type and field spelling" true
    (opts.Ir.fgs_langs
    = [
        {
          Ir.fl_lang = "rust";
          fl_head = Some "FormulaOptions";
          fl_fields = [ ("precision", "Option<u8>") ];
        };
      ])

let error_struct_carries_its_recognition () =
  let _, m = clean () in
  let err =
    List.find
      (fun (s : Ir.shape) -> s.Ir.id = "m#invalid_expression")
      m.Ir.shapes
  in
  let foreign =
    List.find (fun (t : Ir.trait) -> t.Ir.trait_id = "foreign") err.Ir.traits
  in
  match foreign.Ir.value with
  | `List [ go; rust ] ->
      let dec j =
        match Ir_json_extern.decode_foreign_lang j with
        | Ok l -> l
        | Error e -> Alcotest.fail e
      in
      Alcotest.(check bool)
        "go sentinel and field source" true
        (dec go
        = {
            Ir.fl_lang = "go";
            fl_head = Some "ErrParse";
            fl_fields = [ ("message", "Error()") ];
          });
      Alcotest.(check bool)
        "rust pattern and field source" true
        (dec rust
        = {
            Ir.fl_lang = "rust";
            fl_head = Some "Error::Parse";
            fl_fields = [ ("message", "to_string()") ];
          })
  | _ -> Alcotest.fail "expected two language blocks"

(* A wire struct's block: no head, and each entry is the field's Go struct
   tag, verbatim; the field without an entry has none. *)
let wire_struct_carries_its_tags () =
  let _, m = clean () in
  let cal =
    List.find (fun (s : Ir.shape) -> s.Ir.id = "m#calibration") m.Ir.shapes
  in
  let foreign =
    List.find (fun (t : Ir.trait) -> t.Ir.trait_id = "foreign") cal.Ir.traits
  in
  match foreign.Ir.value with
  | `List [ (`Assoc kvs as go) ] ->
      Alcotest.(check bool)
        "no name key without a head" false
        (List.mem_assoc "name" kvs);
      Alcotest.(check bool)
        "go tags, verbatim" true
        (Ir_json_extern.decode_foreign_lang go
        = Ok
            {
              Ir.fl_lang = "go";
              fl_head = None;
              fl_fields = [ ("scale", "env:\"CALC_{profile}_SCALE\"") ];
            })
  | _ -> Alcotest.fail "expected one language block"

let op_lists_its_errors_in_order () =
  let _, m = clean () in
  let ff = List.hd (List.hd m.Ir.ext_libs).Ir.xl_externs in
  Alcotest.(check (list string))
    "errors" [ "m#invalid_expression" ] ff.Ir.x_errors

let prints_idempotently () =
  let file, _ = clean () in
  let printed = Printer.print_file file in
  let reparsed, diags = Parser.parse printed in
  Alcotest.(check int) "no reparse diagnostics" 0 (List.length diags);
  Alcotest.(check string) "idempotent" printed (Printer.print_file reparsed)

(* ── refusals ──────────────────────────────────────────────────────────── *)

let ext_with body =
  Printf.sprintf
    {|ext lib {
  go { #(github.com/x/lib) }
  rust { #(lib) }
%s
}
|} body

let duplicate_language_on_one_struct () =
  Alcotest.(check bool)
    "TC0095" true
    (has "TC0095" (ext_with "  struct h { go { #(A) } go { #(B) } }"))

let unknown_target_named () =
  Alcotest.(check bool)
    "TC0095" true
    (has "TC0095" (ext_with "  struct h { java { #(A) } }"))

let undeclared_module_path_named () =
  let src = ext_with "  struct h { ts { #(A) } }" in
  Alcotest.(check bool) "TC0081" true (has "TC0081" src);
  Alcotest.(check bool) "but a known target" false (has "TC0095" src)

let keyed_entry_on_a_handle () =
  Alcotest.(check bool)
    "TC0097" true
    (has "TC0097" (ext_with "  struct h { go { #(A) x: #(Y) } }"))

let keyed_entry_names_no_field () =
  Alcotest.(check bool)
    "TC0097" true
    (has "TC0097" (ext_with "  struct f { a: string  go { #(A) b: #(B) } }"))

let tagged wire = "pub struct note { id: string  " ^ wire ^ " }"

let head_on_a_wire_struct () =
  Alcotest.(check bool)
    "TC0100: head alone" true
    (has "TC0100" (tagged "go { #(Note) }"));
  Alcotest.(check bool)
    "TC0100: head before the tags" true
    (has "TC0100" (tagged "go { #(Note)  id: #(env:\"ID\") }"));
  Alcotest.(check bool)
    "no TC0092 on a wire struct" false
    (has "TC0092" (tagged "go { #(Note) }"))

let tags_on_a_wire_struct_are_fine () =
  let _, _, _, tc = parse_and_check (tagged "go { id: #(env:\"ID\") }") in
  Alcotest.(check (list string))
    "clean" []
    (List.map (fun (d : Diagnostic.t) -> d.message) tc)

let empty_block_on_a_wire_struct () =
  Alcotest.(check bool) "TC0100" true (has "TC0100" (tagged "go {}"))

let tags_only_go_reads () =
  Alcotest.(check bool)
    "rust: TC0095" true
    (has "TC0095" (tagged "rust { id: #(serde(rename = \"id\")) }"));
  Alcotest.(check bool)
    "ts: TC0095" true
    (has "TC0095" (tagged "ts { id: #(x) }"));
  Alcotest.(check bool)
    "go is the one" false
    (has "TC0095" (tagged "go { id: #(env:\"ID\") }"))

let duplicate_tag_entry () =
  Alcotest.(check bool)
    "TC0097" true
    (has "TC0097" (tagged "go { id: #(env:\"A\")  id: #(env:\"B\") }"))

let block_on_an_entry_or_config () =
  Alcotest.(check bool)
    "config: TC0092" true
    (has "TC0092"
       "pub struct client { id: string @arg  go { id: #(env:\"ID\") } }");
  Alcotest.(check bool)
    "entry: TC0092" true
    (has "TC0092"
       "pub struct client { id: string  go { id: #(env:\"ID\") }  op f(): \
        string }")

let error_struct_without_a_head () =
  Alcotest.(check bool)
    "TC0100" true
    (has "TC0100"
       "@status(404)\n\
        pub struct missing { message: string  go { message: #(Error()) } }")

let ext_struct_without_a_head () =
  Alcotest.(check bool)
    "handle: TC0100" true
    (has "TC0100" (ext_with "  struct h { go {} }"));
  Alcotest.(check bool)
    "form: TC0100" true
    (has "TC0100" (ext_with "  struct f { a: string  go { a: #(A) } }"))

let ext_header_without_a_path () =
  let _, diags = Parser.parse "ext x {\n  go {}\n}" in
  Alcotest.(check bool)
    "a parse diagnostic naming the module path" true
    (List.exists
       (fun (d : Diagnostic.t) ->
         let n = String.length "module path" in
         let m = d.message in
         let rec find i =
           i + n <= String.length m
           && (String.sub m i n = "module path" || find (i + 1))
         in
         find 0)
       diags)

let block_on_an_error_struct_is_fine () =
  let src =
    "@status(404)\n\
     pub struct missing { message: string  go { #(ErrMissing) message: \
     #(Error()) } }"
  in
  Alcotest.(check bool) "no TC0092" false (has "TC0092" src);
  Alcotest.(check bool)
    "unknown field" true
    (has "TC0097"
       "@status(404)\n\
        pub struct missing { message: string  go { #(ErrMissing) detail: \
        #(Error()) } }")

let async_on_go_rejected () =
  let op l =
    ext_with
      (Printf.sprintf
         "  @async(%s)\n\
         \  op f(): string {\n\
         \    go { call: #(F)() }\n\
         \    rust { call: #(f)() }\n\
         \  }"
         l)
  in
  Alcotest.(check bool) "go has no await" true (has "TC0093" (op "go"));
  Alcotest.(check bool)
    "ts declares no module path" true
    (has "TC0093" (op "ts"));
  Alcotest.(check bool) "rust is fine" false (has "TC0093" (op "rust"));
  Alcotest.(check bool)
    "bare @async says nothing" true
    (has "TC0093"
       (ext_with "  @async\n  op f(): string {\n    go { call: #(F)() }\n  }"))

let other_traits_rejected () =
  Alcotest.(check bool)
    "TC0096" true
    (has "TC0096"
       (ext_with
          "  @http(method: \"GET\", path: \"/x\")\n\
          \  op f(): string {\n\
          \    go { call: #(F)() }\n\
          \  }"))

let unknown_error_rejected () =
  Alcotest.(check bool)
    "TC0077" true
    (has "TC0077"
       (ext_with
          "  @errors(nope)\n  op f(): string {\n    go { call: #(F)() }\n  }"))

let fields_and_ops_together_refused () =
  let _, diags =
    Parser.parse
      (ext_with
         "  struct h { a: string\n\
         \    op m(): string { go { call: #(M)() } }\n\
         \  }")
  in
  Alcotest.(check bool)
    "a parse diagnostic" true
    (List.exists
       (fun (d : Diagnostic.t) ->
         let n = String.length "both fields and ops" in
         let m = d.message in
         let rec find i =
           i + n <= String.length m
           && (String.sub m i n = "both fields and ops" || find (i + 1))
         in
         find 0)
       diags)

let () =
  Alcotest.run "lang_block"
    [
      ( "lowering",
        [
          Alcotest.test_case "handle storage type per target" `Quick
            handle_storage_type_per_target;
          Alcotest.test_case "foreign form type and field spelling" `Quick
            foreign_form_type_and_field_spelling;
          Alcotest.test_case "error struct carries its recognition" `Quick
            error_struct_carries_its_recognition;
          Alcotest.test_case "wire struct carries its tags" `Quick
            wire_struct_carries_its_tags;
          Alcotest.test_case "op lists its errors" `Quick
            op_lists_its_errors_in_order;
          Alcotest.test_case "prints idempotently" `Quick prints_idempotently;
        ] );
      ( "refusals",
        [
          Alcotest.test_case "duplicate language" `Quick
            duplicate_language_on_one_struct;
          Alcotest.test_case "unknown target" `Quick unknown_target_named;
          Alcotest.test_case "undeclared module path" `Quick
            undeclared_module_path_named;
          Alcotest.test_case "keyed entry on a handle" `Quick
            keyed_entry_on_a_handle;
          Alcotest.test_case "keyed entry names no field" `Quick
            keyed_entry_names_no_field;
          Alcotest.test_case "head on a wire struct" `Quick
            head_on_a_wire_struct;
          Alcotest.test_case "tags on a wire struct" `Quick
            tags_on_a_wire_struct_are_fine;
          Alcotest.test_case "empty block on a wire struct" `Quick
            empty_block_on_a_wire_struct;
          Alcotest.test_case "only go reads a tag" `Quick tags_only_go_reads;
          Alcotest.test_case "duplicate tag entry" `Quick duplicate_tag_entry;
          Alcotest.test_case "block on an entry or config" `Quick
            block_on_an_entry_or_config;
          Alcotest.test_case "error struct without a head" `Quick
            error_struct_without_a_head;
          Alcotest.test_case "ext struct without a head" `Quick
            ext_struct_without_a_head;
          Alcotest.test_case "ext header without a path" `Quick
            ext_header_without_a_path;
          Alcotest.test_case "block on an error struct" `Quick
            block_on_an_error_struct_is_fine;
          Alcotest.test_case "async on go" `Quick async_on_go_rejected;
          Alcotest.test_case "other traits" `Quick other_traits_rejected;
          Alcotest.test_case "unknown error" `Quick unknown_error_rejected;
          Alcotest.test_case "fields and ops together" `Quick
            fields_and_ops_together_refused;
        ] );
    ]
