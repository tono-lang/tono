open Tono_frontend

(* Compile a project (a list of (module name, source) pairs) and return the model
   plus its diagnostics. *)
let project files = Tono_frontend.compile_project files

let codes files =
  let _, ds = project files in
  List.filter_map (fun (d : Diagnostic.t) -> d.Diagnostic.code) ds

let error_count files =
  let _, ds = project files in
  List.length
    (List.filter (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) ds)

let shape_ids (m : Ir.module_) =
  List.map (fun (s : Ir.shape) -> s.Ir.id) m.shapes

let module_named model name =
  List.find
    (fun (m : Ir.module_) -> String.equal m.Ir.mod_name name)
    model.Ir.modules

(* Substring test for message assertions. *)
let contains ~sub s =
  let n = String.length sub and m = String.length s in
  let rec go i = i + n <= m && (String.sub s i n = sub || go (i + 1)) in
  n = 0 || go 0

let dpos : Span.pos = { line = 0; col = 0; offset = 0 }
let dspan : Span.span = { start = dpos; finish = dpos }

(* ── Import helpers (the qualifier and target derivation) ───────────────── *)

let import ?alias path : Ast.import =
  { Ast.imported_path = path; alias; ispan = dspan }

let qualifier_and_target () =
  (* The qualifier is the alias when present, else the last path segment; the
     target is the dotted path. *)
  Alcotest.(check string)
    "last segment" "common"
    (Modules.qualifier_of (import [ "payments"; "common" ]));
  Alcotest.(check string)
    "alias wins" "c"
    (Modules.qualifier_of (import ~alias:"c" [ "payments"; "common" ]));
  (* A degenerate empty path yields an empty qualifier rather than raising. *)
  Alcotest.(check string) "empty path" "" (Modules.qualifier_of (import []));
  Alcotest.(check string)
    "dotted target" "payments.common"
    (Modules.target_of (import [ "payments"; "common" ]))

(* A qualified type in the calculus resolves to a dotted nominal reference. *)
let calculus_qualified_type () =
  let t = Calc_types.resolve (Ast.TQName ("common", "money", [], dspan)) in
  Alcotest.(check string) "dotted ref" "common.money" (Calc_types.to_string t)

(* ── Qualified resolution across modules ───────────────────────────────── *)

(* A pub struct in one module, referenced qualified from another, resolves to a
   fully-qualified id and both modules compile clean. *)
let cross_module_reference () =
  let common = {|
pub struct money { amount: i64, currency: string }
|} in
  let charge =
    {|
import payments.common

pub struct charge { id: uuid, total: common.money }
|}
  in
  let model, ds =
    project [ ("payments.common", common); ("payments.charge", charge) ]
  in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  let m = module_named model "payments.charge" in
  Alcotest.(check (list string))
    "own id qualified"
    [ "payments.charge#charge" ]
    (shape_ids m);
  match (List.hd m.shapes).kind with
  | Ir.Structure { members = [ _; total ]; _ } ->
      Alcotest.(check bool)
        "reference qualified to the target module" true
        (match total.target with
        | Ir.Ref ("payments.common#money", _) -> true
        | _ -> false)
  | _ -> Alcotest.fail "expected a two-member structure"

(* An import alias supplies the qualifier used in references. *)
let import_alias () =
  let common = "pub struct money { amount: i64 }" in
  let charge =
    {|
import payments.common as c

pub struct charge { total: c.money }
|}
  in
  Alcotest.(check int)
    "alias resolves clean" 0
    (error_count [ ("payments.common", common); ("payments.charge", charge) ])

(* ── Visibility (pub vs private) enforced between modules ───────────────── *)

(* A private (non-pub) shape cannot be referenced from another module: TC0024. *)
let visibility_enforced () =
  let common = "struct money { amount: i64 }" in
  (* not pub *)
  let charge =
    {|
import payments.common

pub struct charge { total: common.money }
|}
  in
  let cs = codes [ ("payments.common", common); ("payments.charge", charge) ] in
  Alcotest.(check bool)
    "not-exported reported" true
    (List.mem Error_codes.not_exported cs)

(* Within a module everything is visible without pub or import. *)
let intra_module_private_ok () =
  let src =
    {|
struct money { amount: i64 }
pub struct charge { total: money }
|}
  in
  Alcotest.(check int)
    "private visible in same module" 0
    (error_count [ ("payments", src) ])

(* ── Imports ───────────────────────────────────────────────────────────── *)

(* A qualified reference without a matching import is an unknown-import error. *)
let missing_import () =
  let charge = "pub struct charge { total: common.money }" in
  let cs = codes [ ("payments.charge", charge) ] in
  Alcotest.(check bool)
    "unknown import reported" true
    (List.mem Error_codes.unknown_import cs)

(* Importing a module that does not exist in the project is an error. *)
let import_of_missing_module () =
  let charge = {|
import nowhere.gone

pub struct charge { x: i64 }
|} in
  let cs = codes [ ("payments.charge", charge) ] in
  Alcotest.(check bool)
    "unknown module reported" true
    (List.mem Error_codes.unknown_import cs)

(* Referencing a name that is pub-imported but absent in the target module is an
   unknown-type error. *)
let unknown_symbol_in_module () =
  let common = "pub struct money { amount: i64 }" in
  let charge =
    {|
import payments.common

pub struct charge { total: common.nonesuch }
|}
  in
  let cs = codes [ ("payments.common", common); ("payments.charge", charge) ] in
  Alcotest.(check bool)
    "unknown type reported" true
    (List.mem Error_codes.unknown_type cs)

(* A generic shape is referenced across a module boundary with the right arity. *)
let cross_module_generic_ok () =
  let common =
    "pub struct money { amount: i64 }\npub struct page[t] { items: []t }"
  in
  let charge =
    {|
import payments.common

pub struct charges { list: common.page[common.money] }
|}
  in
  Alcotest.(check int)
    "generic cross-module resolves" 0
    (error_count [ ("payments.common", common); ("payments.charge", charge) ])

(* A cross-module generic applied with the wrong number of arguments. *)
let cross_module_generic_arity () =
  let common = "pub struct page[t] { items: []t }" in
  let charge = {|
import payments.common

pub struct c { p: common.page }
|} in
  Alcotest.(check bool)
    "arity mismatch reported" true
    (List.mem Error_codes.generic_arity_mismatch
       (codes [ ("payments.common", common); ("payments.charge", charge) ]))

(* Type arguments on a non-generic cross-module shape. *)
let cross_module_non_generic_applied () =
  let common = "pub struct money { amount: i64 }" in
  let charge =
    {|
import payments.common

pub struct c { m: common.money[i64] }
|}
  in
  Alcotest.(check bool)
    "non-generic applied reported" true
    (List.mem Error_codes.non_generic_applied
       (codes [ ("payments.common", common); ("payments.charge", charge) ]))

(* Importing a missing module and referencing through it: the import is reported,
   and the reference itself stays quiet (the root cause is the bad import). *)
let qualified_ref_to_missing_module () =
  let charge = {|
import gone.thing

pub struct c { x: thing.widget }
|} in
  Alcotest.(check bool)
    "missing module import reported" true
    (List.mem Error_codes.unknown_import (codes [ ("proj.charge", charge) ]))

(* The single-file compile path carries no imports, so a qualified reference is an
   unknown import (the default resolver). *)
let single_file_qualified_ref_is_unknown_import () =
  let _, ds =
    Tono_frontend.compile ~module_name:"m" "struct c { x: other.thing }"
  in
  Alcotest.(check bool)
    "no-imports default fires" true
    (List.exists
       (fun (d : Diagnostic.t) ->
         d.Diagnostic.code = Some Error_codes.unknown_import)
       ds)

(* ── Cycle detection (the import graph must be a DAG) ───────────────────── *)

(* Two modules importing each other form a cycle: TC0025. *)
let mutual_cycle () =
  let a = {|
import proj.b

pub struct ta { y: i64 }
|} in
  let b = {|
import proj.a

pub struct tb { x: i64 }
|} in
  let cs = codes [ ("proj.a", a); ("proj.b", b) ] in
  Alcotest.(check bool)
    "cycle reported" true
    (List.mem Error_codes.module_cycle cs)

(* A three-module cycle a -> b -> c -> a is detected. *)
let three_module_cycle () =
  let a = "import proj.b\npub struct ta { x: i64 }" in
  let b = "import proj.c\npub struct tb { x: i64 }" in
  let c = "import proj.a\npub struct tc { x: i64 }" in
  let cs = codes [ ("proj.a", a); ("proj.b", b); ("proj.c", c) ] in
  Alcotest.(check bool)
    "three-way cycle reported" true
    (List.mem Error_codes.module_cycle cs)

(* A self-import is a degenerate cycle. *)
let self_import_cycle () =
  let a = "import proj.a\npub struct ta { x: i64 }" in
  Alcotest.(check bool)
    "self import reported" true
    (List.mem Error_codes.module_cycle (codes [ ("proj.a", a) ]))

(* A diamond (a -> b, a -> c, b -> d, c -> d) is a DAG, not a cycle. *)
let diamond_is_acyclic () =
  let d = "pub struct td { x: i64 }" in
  let b = "import proj.d\npub struct tb { v: d.td }" in
  let c = "import proj.d\npub struct tc { v: d.td }" in
  let a = "import proj.b\nimport proj.c\npub struct ta { l: b.tb, r: c.tc }" in
  Alcotest.(check int)
    "diamond compiles" 0
    (error_count [ ("proj.a", a); ("proj.b", b); ("proj.c", c); ("proj.d", d) ])

(* ── Diagnostics carry the owning module for attribution ───────────────── *)

let diagnostics_labelled_by_module () =
  let charge = "pub struct charge { total: common.money }" in
  let _, ds = project [ ("payments.charge", charge) ] in
  Alcotest.(check bool)
    "message names the module" true
    (List.exists
       (fun (d : Diagnostic.t) -> contains ~sub:"payments.charge:" d.message)
       ds)

let () =
  Alcotest.run "modules"
    [
      ( "helpers",
        [
          Alcotest.test_case "qualifier and target" `Quick qualifier_and_target;
          Alcotest.test_case "calculus qualified type" `Quick
            calculus_qualified_type;
        ] );
      ( "resolution",
        [
          Alcotest.test_case "cross-module reference" `Quick
            cross_module_reference;
          Alcotest.test_case "import alias" `Quick import_alias;
          Alcotest.test_case "unknown symbol in module" `Quick
            unknown_symbol_in_module;
        ] );
      ( "visibility",
        [
          Alcotest.test_case "pub enforced across modules" `Quick
            visibility_enforced;
          Alcotest.test_case "private visible in module" `Quick
            intra_module_private_ok;
        ] );
      ( "imports",
        [
          Alcotest.test_case "missing import" `Quick missing_import;
          Alcotest.test_case "import of missing module" `Quick
            import_of_missing_module;
          Alcotest.test_case "labelled by module" `Quick
            diagnostics_labelled_by_module;
          Alcotest.test_case "generic cross-module ok" `Quick
            cross_module_generic_ok;
          Alcotest.test_case "cross-module arity mismatch" `Quick
            cross_module_generic_arity;
          Alcotest.test_case "cross-module non-generic applied" `Quick
            cross_module_non_generic_applied;
          Alcotest.test_case "qualified ref to missing module" `Quick
            qualified_ref_to_missing_module;
          Alcotest.test_case "single-file qualified ref" `Quick
            single_file_qualified_ref_is_unknown_import;
        ] );
      ( "cycles",
        [
          Alcotest.test_case "mutual cycle" `Quick mutual_cycle;
          Alcotest.test_case "three-module cycle" `Quick three_module_cycle;
          Alcotest.test_case "self import" `Quick self_import_cycle;
          Alcotest.test_case "diamond is acyclic" `Quick diamond_is_acyclic;
        ] );
    ]
