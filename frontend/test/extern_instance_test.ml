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
    ]
