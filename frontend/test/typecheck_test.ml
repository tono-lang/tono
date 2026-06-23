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

(* ── Nullability (TC0007) ──────────────────────────────────────────────── *)

(* A required non-nullable member and a bare nullable member are both fine. *)
let nullability_ok () =
  Alcotest.(check (list string))
    "no codes" []
    (codes "struct s { a: i64 @required, b: i64? }")

let nullability_conflict () =
  Alcotest.(check (list string))
    "@required on a T? member" [ "TC0007" ]
    (codes "struct s { x: i64? @required }")

(* ── Enums (TC0008 / TC0009) ───────────────────────────────────────────── *)

let enum_ok () =
  Alcotest.(check (list string))
    "string- and int-backed enums" []
    (codes "enum colour { red, green }\nenum level { low = 1, high = 2 }")

let enum_duplicate_name () =
  Alcotest.(check (list string))
    "repeated value name" [ "TC0008" ]
    (codes "enum e { a, b, a }")

let enum_duplicate_int () =
  Alcotest.(check (list string))
    "repeated backing value" [ "TC0008" ]
    (codes "enum e { a = 1, b = 1 }")

let enum_backing_mismatch () =
  Alcotest.(check (list string))
    "int-backed case missing its value" [ "TC0009" ]
    (codes "enum e { a = 1, b }")

(* ── Constraint type-compatibility (TC0010) ────────────────────────────── *)

(* Each core constraint on a compatible target lifts with no diagnostic. *)
let constraints_ok () =
  Alcotest.(check (list string))
    "well-typed constraints" []
    (codes
       "struct s {\n\
       \  a: i64 @range(min: 0, max: 9),\n\
       \  b: string @length(min: 1, max: 3),\n\
       \  c: string @pattern(\"^a(b)[c]$\"),\n\
       \  d: i64 @multipleOf(2)\n\
        }")

let range_on_string () =
  Alcotest.(check (list string))
    "@range on a string" [ "TC0010" ]
    (codes "struct s { x: string @range(min: 0, max: 9) }")

let length_on_int () =
  Alcotest.(check (list string))
    "@length on an int" [ "TC0010" ]
    (codes "struct s { x: i64 @length(min: 1) }")

let pattern_on_int () =
  Alcotest.(check (list string))
    "@pattern on an int" [ "TC0010" ]
    (codes "struct s { x: i64 @pattern(\"^a\") }")

let multiple_on_string () =
  Alcotest.(check (list string))
    "@multipleOf on a string" [ "TC0010" ]
    (codes "struct s { x: string @multipleOf(2) }")

(* @length applies to bytes and collections as well as strings. *)
let length_on_bytes () =
  Alcotest.(check (list string))
    "@length on bytes" []
    (codes "struct s { x: bytes @length(min: 1) }")

let length_on_list () =
  Alcotest.(check (list string))
    "@length on a list" []
    (codes "struct s { x: []i64 @length(min: 1) }")

let length_on_map () =
  Alcotest.(check (list string))
    "@length on a map" []
    (codes "struct s { x: map[string]i64 @length(min: 1) }")

(* ── Constraint well-formedness (TC0011) ───────────────────────────────── *)

let range_min_gt_max () =
  Alcotest.(check (list string))
    "@range min > max" [ "TC0011" ]
    (codes "struct s { x: i64 @range(min: 9, max: 0) }")

let length_negative () =
  Alcotest.(check (list string))
    "@length with a negative bound" [ "TC0011" ]
    (codes "struct s { x: string @length(min: -1) }")

let length_inverted () =
  Alcotest.(check (list string))
    "@length min > max" [ "TC0011" ]
    (codes "struct s { x: string @length(min: 3, max: 1) }")

let length_max_negative () =
  Alcotest.(check (list string))
    "@length with a negative max" [ "TC0011" ]
    (codes "struct s { x: string @length(max: -1) }")

let multiple_non_positive () =
  Alcotest.(check (list string))
    "@multipleOf of zero" [ "TC0011" ]
    (codes "struct s { x: i64 @multipleOf(0) }")

let pattern_empty () =
  Alcotest.(check (list string))
    "empty @pattern" [ "TC0011" ]
    (codes "struct s { x: string @pattern(\"\") }")

let pattern_unbalanced () =
  Alcotest.(check (list string))
    "unbalanced @pattern" [ "TC0011" ]
    (codes "struct s { x: string @pattern(\"a(b\") }")

let pattern_unmatched_paren () =
  Alcotest.(check (list string))
    "a close paren with nothing open" [ "TC0011" ]
    (codes "struct s { x: string @pattern(\"a)b\") }")

let pattern_unmatched_bracket () =
  Alcotest.(check (list string))
    "a close bracket with nothing open" [ "TC0011" ]
    (codes "struct s { x: string @pattern(\"a]b\") }")

(* ── Default type (TC0012) ─────────────────────────────────────────────── *)

(* A default whose type matches the member (including a non-scalar target, which
   v1 accepts without deep typing) carries no diagnostic. *)
let default_ok () =
  Alcotest.(check (list string))
    "well-typed defaults" []
    (codes
       "struct s { a: i64 @default(3), b: string @default(\"x\"), c: bool \
        @default(true) }")

let default_string_on_int () =
  Alcotest.(check (list string))
    "string default on an int" [ "TC0012" ]
    (codes "struct s { x: i64 @default(\"x\") }")

let default_int_on_string () =
  Alcotest.(check (list string))
    "int default on a string" [ "TC0012" ]
    (codes "struct s { x: string @default(5) }")

let default_string_on_float () =
  Alcotest.(check (list string))
    "string default on a float" [ "TC0012" ]
    (codes "struct s { x: float @default(\"x\") }")

(* An int literal is a valid float default. *)
let default_int_on_float () =
  Alcotest.(check (list string))
    "int default on a float" []
    (codes "struct s { x: float @default(3) }")

(* ── Default constraint satisfaction (TC0013) ──────────────────────────── *)

(* A default within its range and a float default exercise the numeric path. *)
let default_in_range () =
  Alcotest.(check (list string))
    "default inside the range" []
    (codes "struct s { x: i64 @range(min: 0, max: 9) @default(5) }")

let default_below_range () =
  Alcotest.(check (list string))
    "default below the range" [ "TC0013" ]
    (codes "struct s { x: i64 @range(min: 10) @default(5) }")

let default_float_above_range () =
  Alcotest.(check (list string))
    "float default above the range" [ "TC0013" ]
    (codes "struct s { x: float @range(min: 0, max: 1) @default(2.5) }")

let default_not_multiple () =
  Alcotest.(check (list string))
    "default not a multiple" [ "TC0013" ]
    (codes "struct s { x: i64 @multipleOf(3) @default(5) }")

let default_length_violation () =
  Alcotest.(check (list string))
    "default longer than @length max" [ "TC0013" ]
    (codes "struct s { x: string @length(max: 2) @default(\"abcd\") }")

let default_length_too_short () =
  Alcotest.(check (list string))
    "default shorter than @length min" [ "TC0013" ]
    (codes "struct s { x: string @length(min: 3) @default(\"ab\") }")

(* A @pattern with a default is left unchecked (no regex engine), so a default
   that matches the type stays silent. *)
let default_with_pattern () =
  Alcotest.(check (list string))
    "default alongside a pattern" []
    (codes "struct s { x: string @pattern(\"^a\") @default(\"anything\") }")

(* A default that satisfies @multipleOf and one whose length is within @length
   bounds both stay silent. *)
let default_multiple_ok () =
  Alcotest.(check (list string))
    "default that is a multiple" []
    (codes "struct s { x: i64 @multipleOf(3) @default(6) }")

let default_length_ok () =
  Alcotest.(check (list string))
    "default within length bounds" []
    (codes "struct s { x: string @length(min: 1, max: 5) @default(\"ab\") }")

(* Single-sided bounds exercise the absent-bound (None) arms of the checks. *)
let default_range_max_only () =
  Alcotest.(check (list string))
    "default under a max-only range" []
    (codes "struct s { x: i64 @range(max: 10) @default(5) }")

let default_length_min_only () =
  Alcotest.(check (list string))
    "default over a min-only length" []
    (codes "struct s { x: string @length(min: 1) @default(\"abc\") }")

(* When the constraint is itself type-incompatible (TC0010), a type-matched
   default cannot be range/length-checked, so only the constraint error stands. *)
let default_non_numeric_with_range () =
  Alcotest.(check (list string))
    "string default under a misapplied @range" [ "TC0010" ]
    (codes "struct s { x: string @range(min: 0) @default(\"a\") }")

let default_numeric_with_length () =
  Alcotest.(check (list string))
    "int default under a misapplied @length" [ "TC0010" ]
    (codes "struct s { x: i64 @length(min: 1) @default(5) }")

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
          Alcotest.test_case "one arg arity two" `Quick
            generic_one_arg_arity_two;
          Alcotest.test_case "non-generic applied" `Quick non_generic_applied;
          Alcotest.test_case "type param applied" `Quick type_param_applied;
        ] );
      ( "duplicate",
        [ Alcotest.test_case "duplicate shape" `Quick duplicate_shape ] );
      ( "nullability",
        [
          Alcotest.test_case "nullability ok" `Quick nullability_ok;
          Alcotest.test_case "nullability conflict" `Quick nullability_conflict;
        ] );
      ( "enums",
        [
          Alcotest.test_case "enum ok" `Quick enum_ok;
          Alcotest.test_case "duplicate name" `Quick enum_duplicate_name;
          Alcotest.test_case "duplicate int" `Quick enum_duplicate_int;
          Alcotest.test_case "backing mismatch" `Quick enum_backing_mismatch;
        ] );
      ( "constraints",
        [
          Alcotest.test_case "constraints ok" `Quick constraints_ok;
          Alcotest.test_case "range on string" `Quick range_on_string;
          Alcotest.test_case "length on int" `Quick length_on_int;
          Alcotest.test_case "pattern on int" `Quick pattern_on_int;
          Alcotest.test_case "multipleOf on string" `Quick multiple_on_string;
          Alcotest.test_case "length on bytes" `Quick length_on_bytes;
          Alcotest.test_case "length on list" `Quick length_on_list;
          Alcotest.test_case "length on map" `Quick length_on_map;
          Alcotest.test_case "range min>max" `Quick range_min_gt_max;
          Alcotest.test_case "length negative" `Quick length_negative;
          Alcotest.test_case "length max negative" `Quick length_max_negative;
          Alcotest.test_case "length inverted" `Quick length_inverted;
          Alcotest.test_case "multipleOf non-positive" `Quick
            multiple_non_positive;
          Alcotest.test_case "pattern empty" `Quick pattern_empty;
          Alcotest.test_case "pattern unbalanced" `Quick pattern_unbalanced;
          Alcotest.test_case "pattern unmatched paren" `Quick
            pattern_unmatched_paren;
          Alcotest.test_case "pattern unmatched bracket" `Quick
            pattern_unmatched_bracket;
        ] );
      ( "defaults",
        [
          Alcotest.test_case "default ok" `Quick default_ok;
          Alcotest.test_case "string default on int" `Quick
            default_string_on_int;
          Alcotest.test_case "int default on string" `Quick
            default_int_on_string;
          Alcotest.test_case "string default on float" `Quick
            default_string_on_float;
          Alcotest.test_case "int default on float" `Quick default_int_on_float;
          Alcotest.test_case "default in range" `Quick default_in_range;
          Alcotest.test_case "default below range" `Quick default_below_range;
          Alcotest.test_case "float default above range" `Quick
            default_float_above_range;
          Alcotest.test_case "default not multiple" `Quick default_not_multiple;
          Alcotest.test_case "default length violation" `Quick
            default_length_violation;
          Alcotest.test_case "default length too short" `Quick
            default_length_too_short;
          Alcotest.test_case "default with pattern" `Quick default_with_pattern;
          Alcotest.test_case "default multiple ok" `Quick default_multiple_ok;
          Alcotest.test_case "default length ok" `Quick default_length_ok;
          Alcotest.test_case "default range max only" `Quick
            default_range_max_only;
          Alcotest.test_case "default length min only" `Quick
            default_length_min_only;
          Alcotest.test_case "string default with range" `Quick
            default_non_numeric_with_range;
          Alcotest.test_case "int default with length" `Quick
            default_numeric_with_length;
        ] );
    ]
