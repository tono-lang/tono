open Tono_frontend

(* Typecheck coverage for map-index match subjects (`[.seg]` -> T?), the
   mandatory `null` arm, and the `._` subject reference. Split out of
   entries_edge_test.ml to keep that file under the line-count ceiling. *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

let expect what wanted src =
  Alcotest.(check (list string)) what wanted (codes src)

let wire = "struct r { y: string }\n"

let entry fields =
  "pub struct c {\n" ^ fields
  ^ "\n\
    \  ep: string @env(\"EP\")\n\
    \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
     }\n" ^ wire

let map_index_fields =
  "  by_segment: map[string]string @env(\"BS\")\n  seg: string @env(\"SEG\")\n"

let map_index_resolves_optional_with_null_arm () =
  expect "map index resolves T? with a null arm" []
    (entry
       (map_index_fields
      ^ "  x: string = match .by_segment[.seg] { null => \"none\" _ => ._ }"))

let map_index_missing_null_arm () =
  expect "optional subject missing null arm" [ "TC0089" ]
    (entry
       (map_index_fields ^ "  x: string = match .by_segment[.seg] { _ => ._ }"))

let null_arm_on_non_optional_subject () =
  expect "null arm on a non-optional subject" [ "TC0090" ]
    (entry
       "  v: string @env(\"V\")\n\
       \  x: string = match .v { null => \"a\" _ => \"b\" }")

let subject_ref_inside_null_arm () =
  expect "'._' inside the null arm" [ "TC0091" ]
    (entry
       (map_index_fields
      ^ "  x: string = match .by_segment[.seg] { null => ._ _ => \"x\" }"))

let subject_ref_narrowed_type_mismatch () =
  expect "'._' has the map's value type, not the field's" [ "TC0040" ]
    (entry
       ("  by_segment: map[string]i32 @env(\"BS\")\n\
        \  seg: string @env(\"SEG\")\n"
      ^ "  x: string = match .by_segment[.seg] { null => \"none\" _ => ._ }"))

let subject_ref_outside_match_is_unknown_field () =
  (* "._" is only meaningful in a match arm's value position; the parser
     stays permissive and lets it through as an ordinary ".{_}" ref
     anywhere else a ref is legal, which the typechecker then rejects like
     any other unresolvable field. *)
  expect "'._' outside a match arm" [ "TC0038" ]
    (entry "  x: string @env(._)\n")

let map_index_key_type_mismatch () =
  expect "map index key of the wrong type" [ "TC0088" ]
    (entry
       ("  by_segment: map[string]string @env(\"BS\")\n\
        \  seg: i32 @env(\"SEG\")\n"
      ^ "  x: string = match .by_segment[.seg] { null => \"none\" _ => ._ }"))

let map_index_exhaustiveness_unaffected_by_null () =
  (* A "null" arm never counts toward string/int coverage, and is checked
     separately: both diagnostics fire independently here. *)
  expect "string exhaustiveness still needs '_', separate from 'null'"
    [ "TC0041"; "TC0089" ]
    (entry
       (map_index_fields
      ^ "  x: string = match .by_segment[.seg] { \"a\" => \"1\" }"))

let () =
  Alcotest.run "entries_map_index"
    [
      ( "match",
        [
          Alcotest.test_case "map index resolves optional, has null arm" `Quick
            map_index_resolves_optional_with_null_arm;
          Alcotest.test_case "map index missing null arm" `Quick
            map_index_missing_null_arm;
          Alcotest.test_case "null arm on non-optional subject" `Quick
            null_arm_on_non_optional_subject;
          Alcotest.test_case "'._' inside the null arm" `Quick
            subject_ref_inside_null_arm;
          Alcotest.test_case "'._' narrowed type mismatch" `Quick
            subject_ref_narrowed_type_mismatch;
          Alcotest.test_case "'._' outside a match arm" `Quick
            subject_ref_outside_match_is_unknown_field;
          Alcotest.test_case "map index key type mismatch" `Quick
            map_index_key_type_mismatch;
          Alcotest.test_case "map index exhaustiveness unaffected by null"
            `Quick map_index_exhaustiveness_unaffected_by_null;
        ] );
    ]
