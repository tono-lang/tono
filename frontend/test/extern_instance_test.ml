open Tono_frontend

(* Typecheck and printer coverage for an opaque foreign type's instantiation
   clause (type Name("Foreign", Arg) { ... }). Split out of
   extern_typecheck_test.ml to keep that file under the line-count ceiling. *)

let contains ~sub s =
  let n = String.length sub and m = String.length s in
  let rec go i = i + n <= m && (String.sub s i n = sub || go (i + 1)) in
  n = 0 || go 0

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)
let has code src = List.mem code (codes src)

let line_of code src =
  let d =
    List.find (fun (d : Diagnostic.t) -> d.code = Some code) (check src)
  in
  d.span.start.line

(* The same (foreign name, argument) instantiation declared twice, across two
   handles in the same "ext" block, is TC0092, spanning the second
   declaration (not the first, so the diagnostic points at the redundant
   one). *)
let instance_duplicate_has_a_span_on_the_second_occurrence () =
  let src =
    {|ext cfgkit {
  go: "github.com/x/cfgkit"

  type first_source("Source", settings) {
    extern get(): settings { go { call: "Get"() } }
  }

  type second_source("Source", settings) {
    extern get(): settings { go { call: "Get"() } }
  }
}

struct settings { endpoint: string }
|}
  in
  Alcotest.(check bool) "duplicate instantiation" true (has "TC0092" src);
  Alcotest.(check int) "span on the second declaration" 8 (line_of "TC0092" src)

(* Two different arguments (or two different foreign names) are two distinct
   instantiations, not a collision. *)
let instance_distinct_arguments_no_collision () =
  let src =
    {|ext cfgkit {
  go: "github.com/x/cfgkit"

  type settings_source("Source", settings) {
    extern get(): settings { go { call: "Get"() } }
  }

  type flags_source("Source", flags) {
    extern get(): flags { go { call: "Get"() } }
  }
}

struct settings { endpoint: string }
struct flags { enabled: bool }
|}
  in
  Alcotest.(check bool) "no duplicate instantiation" false (has "TC0092" src)

(* The same foreign name and argument in two *different* "ext" libraries is
   not a collision: the foreign name belongs to the library, not the user,
   so two unrelated libraries that each happen to export a "Source" (a
   plausible, common name) must not be forced into a false conflict. *)
let instance_same_name_different_libs_no_collision () =
  let src =
    {|ext liba {
  go: "github.com/x/liba"

  type source("Source", settings) {
    extern get(): settings { go { call: "Get"() } }
  }
}

ext libb {
  go: "github.com/x/libb"

  type source("Source", settings) {
    extern get(): settings { go { call: "Get"() } }
  }
}

struct settings { endpoint: string }
|}
  in
  Alcotest.(check bool)
    "no false collision across libraries" false (has "TC0092" src)

(* The instantiation argument must be a tono type already declared in the
   module: a foreign struct name is not one (it is never "known" outside its
   own ext block for this purpose), so it falls through to the ordinary
   unresolved-name diagnostic (TC0001), with a span on the argument itself. *)
let instance_arg_not_a_declared_type () =
  let src =
    {|ext cfgkit {
  go: "github.com/x/cfgkit"

  type env_source("Source", nowhere) {
    extern get(): nowhere { go { call: "Get"() } }
  }
}
|}
  in
  Alcotest.(check bool) "unknown instance argument" true (has "TC0001" src);
  Alcotest.(check int) "span on the argument" 4 (line_of "TC0001" src)

(* The instantiation clause round-trips through the printer unchanged: it is
   part of the "fmt" surface, not just parsed and dropped. *)
let instance_clause_prints_idempotently () =
  let src =
    {|ext cfgkit {
  go: "github.com/x/cfgkit"

  type env_source("Source", settings) {
    extern get(): settings {
      go { call: "Get"() }
    }
  }
}

struct settings { endpoint: string }
|}
  in
  let file, _ = Parser.parse src in
  let printed = Printer.print_file file in
  Alcotest.(check bool)
    "instance clause survives printing" true
    (contains ~sub:"(\"Source\", settings)" printed);
  let reparsed, _ = Parser.parse printed in
  let printed2 = Printer.print_file reparsed in
  Alcotest.(check string) "idempotent" printed printed2

(* A foreign type that is not generic keeps parsing and checking exactly as
   before: no instance clause, no boundary diagnostic drawn by this feature. *)
let instance_absent_for_a_non_generic_handle () =
  let src =
    {|ext bus {
  go: "github.com/x/bus"

  type publisher {
    extern send(topic: string): string { go { call: "Send"(topic) } }
  }
}
|}
  in
  Alcotest.(check bool) "no instance diagnostics" false (has "TC0092" src)

(* The "interface" marker parses in its position (after the name or the
   instantiation clause), lands on the AST and the lowered IR, and
   round-trips through the printer: it is declaration surface, not a trait,
   so fmt must keep it. *)
let interface_marker_parses_lowers_and_prints () =
  let src =
    {|ext calckit {
  go: "github.com/x/calckit"

  type meter("Meter", float) interface {
    extern read(): float {
      go { call: "Read"() }
    }
  }
}
|}
  in
  let file, errs = Parser.parse src in
  Alcotest.(check int)
    "parses clean" 0
    (List.length
       (List.filter
          (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error)
          errs));
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let lib = List.hd m.Ir.ext_libs in
  let t = List.hd lib.Ir.xl_types in
  Alcotest.(check bool) "lowered as interface" true t.Ir.opq_interface;
  let printed = Printer.print_file file in
  Alcotest.(check bool)
    "marker survives printing" true
    (contains ~sub:{|("Meter", float) interface {|} printed);
  let reparsed, _ = Parser.parse printed in
  let printed2 = Printer.print_file reparsed in
  Alcotest.(check string) "idempotent" printed printed2

(* Without the marker nothing changes: the handle lowers as concrete, which
   is what every declaration written before the marker existed means. *)
let interface_absent_lowers_as_concrete () =
  let src =
    {|ext bus {
  go: "github.com/x/bus"

  type publisher {
    extern send(topic: string): string { go { call: "Send"(topic) } }
  }
}
|}
  in
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let lib = List.hd m.Ir.ext_libs in
  let t = List.hd lib.Ir.xl_types in
  Alcotest.(check bool) "concrete by default" false t.Ir.opq_interface

(* The keyed form: one foreign name per language, in the same "lang: string"
   shape the ext header's module paths use. It parses, lowers verbatim (one
   IR entry per written language), and round-trips through the printer. *)
let keyed_names_parse_lower_and_print () =
  let src =
    {|ext calckit {
  go: "github.com/x/calckit"
  rust: "calckit"

  type meter(go: "Meter", rust: "Gauge", float) interface {
    extern read(): float {
      go { call: "Read"() }
      rust { call: "read"() }
    }
  }
}
|}
  in
  let file, errs = Parser.parse src in
  Alcotest.(check int)
    "parses clean" 0
    (List.length
       (List.filter
          (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error)
          errs));
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let lib = List.hd m.Ir.ext_libs in
  let t = List.hd lib.Ir.xl_types in
  (match t.Ir.opq_instance with
  | Some { inst_names; _ } ->
      Alcotest.(check (list (pair string string)))
        "one entry per written language"
        [ ("go", "Meter"); ("rust", "Gauge") ]
        (List.map
           (fun (n : Ir.instance_name) -> (n.inn_lang, n.inn_name))
           inst_names)
  | None -> Alcotest.fail "expected an instance");
  let printed = Printer.print_file file in
  Alcotest.(check bool)
    "keyed clause survives printing" true
    (contains ~sub:{|(go: "Meter", rust: "Gauge", float) interface {|} printed);
  let reparsed, _ = Parser.parse printed in
  let printed2 = Printer.print_file reparsed in
  Alcotest.(check string) "idempotent" printed printed2

(* The shared form expands in the IR to one entry per declared language, so
   backends only ever look a language up. *)
let shared_name_expands_over_declared_languages () =
  let src =
    {|ext calckit {
  go: "github.com/x/calckit"
  rust: "calckit"

  type meter("Meter", float) {
    extern read(): float {
      go { call: "Read"() }
      rust { call: "read"() }
    }
  }
}
|}
  in
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let lib = List.hd m.Ir.ext_libs in
  let t = List.hd lib.Ir.xl_types in
  match t.Ir.opq_instance with
  | Some { inst_names; _ } ->
      Alcotest.(check (list (pair string string)))
        "expanded over declared languages"
        [ ("go", "Meter"); ("rust", "Meter") ]
        (List.map
           (fun (n : Ir.instance_name) -> (n.inn_lang, n.inn_name))
           inst_names)
  | None -> Alcotest.fail "expected an instance"

(* A keyed list must be exactly one entry per declared language: a language
   named twice, a language with no declared module path, and a declared
   language left without a name are each TC0095. *)
let keyed_names_mismatches_are_diagnosed () =
  let dup =
    {|ext calckit {
  go: "github.com/x/calckit"

  type meter(go: "Meter", go: "Gauge", float) {
    extern read(): float { go { call: "Read"() } }
  }
}
|}
  in
  Alcotest.(check bool) "duplicate language" true (has "TC0095" dup);
  let unknown =
    {|ext calckit {
  go: "github.com/x/calckit"

  type meter(go: "Meter", rust: "Gauge", float) {
    extern read(): float { go { call: "Read"() } }
  }
}
|}
  in
  Alcotest.(check bool) "undeclared language" true (has "TC0095" unknown);
  let missing =
    {|ext calckit {
  go: "github.com/x/calckit"
  rust: "calckit"

  type meter(go: "Meter", float) {
    extern read(): float {
      go { call: "Read"() }
      rust { call: "read"() }
    }
  }
}
|}
  in
  Alcotest.(check bool) "missing declared language" true (has "TC0095" missing)

(* A keyed instantiation collides with a shared one when they claim the same
   foreign name and argument for the same language of the same ext. *)
let keyed_and_shared_names_collide_per_language () =
  let src =
    {|ext cfgkit {
  go: "github.com/x/cfgkit"

  type first_source("Source", settings) {
    extern get(): settings { go { call: "Get"() } }
  }

  type second_source(go: "Source", settings) {
    extern get(): settings { go { call: "Get"() } }
  }
}

struct settings { endpoint: string }
|}
  in
  Alcotest.(check bool) "collides per language" true (has "TC0092" src)

(* Different per-language names for the same argument are distinct
   instantiations: no collision. *)
let keyed_names_differing_no_collision () =
  let src =
    {|ext cfgkit {
  go: "github.com/x/cfgkit"

  type first_source(go: "Source", settings) {
    extern get(): settings { go { call: "Get"() } }
  }

  type second_source(go: "Registry", settings) {
    extern get(): settings { go { call: "Get"() } }
  }
}

struct settings { endpoint: string }
|}
  in
  Alcotest.(check bool) "no collision" false (has "TC0092" src)

let () =
  Alcotest.run "extern_instance"
    [
      ( "instantiation",
        [
          Alcotest.test_case "duplicate has a span" `Quick
            instance_duplicate_has_a_span_on_the_second_occurrence;
          Alcotest.test_case "distinct arguments" `Quick
            instance_distinct_arguments_no_collision;
          Alcotest.test_case "same name different libs" `Quick
            instance_same_name_different_libs_no_collision;
          Alcotest.test_case "arg not declared" `Quick
            instance_arg_not_a_declared_type;
          Alcotest.test_case "prints idempotently" `Quick
            instance_clause_prints_idempotently;
          Alcotest.test_case "absent for non-generic" `Quick
            instance_absent_for_a_non_generic_handle;
        ] );
      ( "per-language names",
        [
          Alcotest.test_case "keyed form parses, lowers and prints" `Quick
            keyed_names_parse_lower_and_print;
          Alcotest.test_case "shared name expands" `Quick
            shared_name_expands_over_declared_languages;
          Alcotest.test_case "mismatches are diagnosed" `Quick
            keyed_names_mismatches_are_diagnosed;
          Alcotest.test_case "keyed and shared collide" `Quick
            keyed_and_shared_names_collide_per_language;
          Alcotest.test_case "different names do not collide" `Quick
            keyed_names_differing_no_collision;
        ] );
      ( "interface marker",
        [
          Alcotest.test_case "parses, lowers and prints" `Quick
            interface_marker_parses_lowers_and_prints;
          Alcotest.test_case "absent means concrete" `Quick
            interface_absent_lowers_as_concrete;
        ] );
    ]
