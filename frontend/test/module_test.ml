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
        ] );
      ( "cycles",
        [
          Alcotest.test_case "mutual cycle" `Quick mutual_cycle;
          Alcotest.test_case "three-module cycle" `Quick three_module_cycle;
          Alcotest.test_case "self import" `Quick self_import_cycle;
          Alcotest.test_case "diamond is acyclic" `Quick diamond_is_acyclic;
        ] );
    ]
