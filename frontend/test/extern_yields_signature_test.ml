open Tono_frontend

(* The two readings of a yields: list, told apart by returns: (the rule
   [Check_ext_lib.check_yields_consumption] and [check_returns_not_handle]
   carry). With no returns:, the list is the call's whole signature and a
   position typed as the op's own return is consumed by that return: the
   binding a handle constructor writes when the library returns only the
   handle. With a returns:, the positions name what to project from. A
   returns: never builds an opaque handle (TC0099). Split out of
   [extern_typecheck_test.ml] to keep it under the line-count cap; the
   helpers mirror its own. *)

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

let parse_diag_count src =
  let _, pdiags = Parser.parse src in
  List.length pdiags

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)
let has code src = List.mem code (codes src)

(* The one binding a handle constructor writes when the library returns
   only the handle: a single position typed as the op's own return, no
   returns:. The position is consumed by the return itself, the list is the
   call's whole signature, and nothing is dead. The sibling op keeps the
   convention by omitting yields:, so both readings sit in one block. *)
let handle_source =
  {|ext kit {
  go { #(github.com/x/kit) }

  struct session {
    go { #(*Client) }

    op ping(): string {
      go { call: #(Ping)() }
    }
  }

  op dial(addr: string): session {
    go {
      call: #(Dial)(addr)
      yields: (c: session)
    }
  }

  op open(addr: string): session {
    go { call: #(Open)(addr) }
  }
}
|}

let yields_position_that_is_the_return_is_consumed () =
  Alcotest.(check int) "parses clean" 0 (parse_diag_count handle_source);
  Alcotest.(check (list string))
    "no yields/returns diagnostics" [] (codes handle_source)

(* A second position next to the one the return consumes still has nothing
   reading it. *)
let yields_position_beside_the_return_is_dead () =
  let src =
    Str.global_replace
      (Str.regexp_string "yields: (c: session)")
      "yields: (c: session, n: i64)" handle_source
  in
  Alcotest.(check bool) "dead sibling position" true (has "TC0072" src);
  let d =
    List.find (fun (d : Diagnostic.t) -> d.code = Some "TC0072") (check src)
  in
  Alcotest.(check bool)
    "names the position and the op's return" true
    (contains ~sub:"position 'n'" d.message
    && contains ~sub:"the op's own return 'session'" d.message)

(* A position typed as the op's return consumed by a returns: is still the
   projection role: nothing about the position itself turns returns: off.
   Here the return is an ordinary shape, so returns: is legal and the
   position is read through it. *)
let yields_position_of_the_return_type_read_by_returns () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: app_config)
      returns: app_config { endpoint: .cfg.endpoint }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check (list string)) "no diagnostics" [] (codes src)

(* ── TC0099: returns: cannot build an opaque handle ─────────────────────── *)

(* Writing a returns: for the handle "consumes" the position on paper and
   the frontend used to accept it, while no target could project a handle
   from anything. It is refused by name now. *)
let returns_building_a_handle_is_refused () =
  let src =
    Str.global_replace
      (Str.regexp_string "yields: (c: session)")
      "yields: (c: session)\n      returns: session { c: .c }" handle_source
  in
  Alcotest.(check bool) "returns onto a handle" true (has "TC0099" src);
  Alcotest.(check bool) "no dead-position noise" false (has "TC0072" src);
  let d =
    List.find (fun (d : Diagnostic.t) -> d.code = Some "TC0099") (check src)
  in
  Alcotest.(check bool)
    "names the handle" true
    (contains ~sub:"'session', an opaque handle" d.message)

let () =
  Alcotest.run "extern_yields_signature"
    [
      ( "signature",
        [
          Alcotest.test_case "yields position that is the return" `Quick
            yields_position_that_is_the_return_is_consumed;
          Alcotest.test_case "yields position beside the return is dead" `Quick
            yields_position_beside_the_return_is_dead;
          Alcotest.test_case
            "yields position of the return type read by returns" `Quick
            yields_position_of_the_return_type_read_by_returns;
        ] );
      ( "returns-onto-handle",
        [
          Alcotest.test_case "returns building a handle" `Quick
            returns_building_a_handle_is_refused;
        ] );
    ]
