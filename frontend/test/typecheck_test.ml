open Tono_frontend

(* Parse + lower a snippet, then run the typecheck pass, returning its
   diagnostics (the pass is exposed directly; it is not yet wired into compile). *)
let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

(* ── Reference resolution (TC0001) ─────────────────────────────────────── *)

(* Primitives, a declared ref, list/map/nullable, a generic parameter, an enum,
   a union (payload-bearing and bare variants), and an operation all resolve with
   no diagnostics. *)
let clean () =
  Alcotest.(check (list string))
    "no codes" []
    (codes
       "struct a { x: i64 }\n\
        struct b { y: a, ys: []a, m: map[string]a, n: a? }\n\
        struct page[t] { items: []t }\n\
        enum e { x, y }\n\
        union u { bare, payload(a) }\n\
        op o(a): a")

let unknown_type () =
  Alcotest.(check (list string))
    "unknown ref" [ "TC0001" ]
    (codes "struct a { x: nope }")

let unknown_in_list () =
  Alcotest.(check (list string))
    "unknown inside list" [ "TC0001" ]
    (codes "struct a { xs: []nope }")

let unknown_generic_arg () =
  Alcotest.(check (list string))
    "unknown generic arg" [ "TC0001" ]
    (codes "struct a { p: page[nope] }\nstruct page[t] { items: []t }")

let unknown_in_op () =
  Alcotest.(check (list string))
    "unknown op output" [ "TC0001" ] (codes "op o(): nope")

let union_payload_resolved () =
  Alcotest.(check (list string))
    "union payload resolved" [ "TC0001" ]
    (codes "union u { v(nope) }")

(* A malformed type (a parse error already reported it) carries no name to
   resolve, so the pass stays silent rather than double-reporting. *)
let malformed_type_silent () =
  Alcotest.(check (list string)) "no codes" [] (codes "struct a { x: }")

(* ── Generic arity (TC0004) ────────────────────────────────────────────── *)

(* A generic shape applied with exactly its arity resolves cleanly. *)
let generic_applied_ok () =
  Alcotest.(check (list string))
    "well-applied generic" []
    (codes "struct page[t] { items: []t }\nstruct s { p: page[i64] }")

let generic_too_few () =
  Alcotest.(check (list string))
    "generic used bare" [ "TC0004" ]
    (codes "struct page[t] { items: []t }\nstruct s { p: page }")

let generic_too_many () =
  Alcotest.(check (list string))
    "two args for arity one" [ "TC0004" ]
    (codes "struct page[t] { items: []t }\nstruct s { p: page[i64, string] }")

(* One argument for an arity-two shape exercises the singular wording branch. *)
let generic_one_arg_arity_two () =
  Alcotest.(check (list string))
    "one arg for arity two" [ "TC0004" ]
    (codes "struct pair[a, b] { x: a, y: b }\nstruct s { p: pair[i64] }")

(* ── Non-generic application (TC0005) ──────────────────────────────────── *)

let non_generic_applied () =
  Alcotest.(check (list string))
    "args on a non-generic shape" [ "TC0005" ]
    (codes "struct a { x: i64 }\nstruct s { y: a[i64] }")

let type_param_applied () =
  Alcotest.(check (list string))
    "args on a type parameter" [ "TC0005" ]
    (codes "struct box[t] { x: t[i64] }")

(* ── Duplicates (TC0002) ───────────────────────────────────────────────── *)

let duplicate_shape () =
  Alcotest.(check (list string))
    "duplicate" [ "TC0002" ]
    (codes "struct a { x: i64 }\nstruct a { y: i64 }")

let () =
  Alcotest.run "typecheck"
    [
      ( "resolve",
        [
          Alcotest.test_case "clean" `Quick clean;
          Alcotest.test_case "unknown type" `Quick unknown_type;
          Alcotest.test_case "unknown in list" `Quick unknown_in_list;
          Alcotest.test_case "unknown generic arg" `Quick unknown_generic_arg;
          Alcotest.test_case "unknown in op" `Quick unknown_in_op;
          Alcotest.test_case "union payload" `Quick union_payload_resolved;
          Alcotest.test_case "malformed type" `Quick malformed_type_silent;
        ] );
      ( "generics",
        [
          Alcotest.test_case "generic applied ok" `Quick generic_applied_ok;
          Alcotest.test_case "generic too few" `Quick generic_too_few;
          Alcotest.test_case "generic too many" `Quick generic_too_many;
          Alcotest.test_case "one arg arity two" `Quick generic_one_arg_arity_two;
          Alcotest.test_case "non-generic applied" `Quick non_generic_applied;
          Alcotest.test_case "type param applied" `Quick type_param_applied;
        ] );
      ( "duplicate",
        [ Alcotest.test_case "duplicate shape" `Quick duplicate_shape ] );
    ]
