(* Constraint and default validation. The lowered IR already holds each member's
   resolved [target], its lifted core [constraints], and its [default] as JSON;
   this pass checks those for type-compatibility (TC0010), well-formedness
   (TC0011), and a default that matches the type (TC0012) and satisfies the
   constraints (TC0013). The IR carries no spans, so diagnostics borrow the
   member's name span from the surface AST, which keeps members in declaration
   order through lowering. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* ── Target classification ─────────────────────────────────────────────── *)

let is_numeric = function Ir.Prim (Ir.Int _ | Ir.Float) -> true | _ -> false
let is_string = function Ir.Prim Ir.String -> true | _ -> false

(* @length applies to anything with a length: text, bytes, and collections. *)
let is_lengthy = function
  | Ir.Prim (Ir.String | Ir.Bytes) | Ir.List _ | Ir.Map _ -> true
  | _ -> false

(* ── Regex syntactic sanity (TC0011) ───────────────────────────────────── *)

(* A full ECMA-262 validation is out of scope (no regex dependency); this only
   rejects the obviously broken: an empty pattern or unbalanced grouping. The
   lexer strips string escapes, so no backslash reaches here to consider. *)
let balanced (s : string) : bool =
  let paren = ref 0 and brack = ref 0 and ok = ref true in
  String.iter
    (fun c ->
      match c with
      | '(' -> incr paren
      | ')' -> if !paren = 0 then ok := false else decr paren
      | '[' -> incr brack
      | ']' -> if !brack = 0 then ok := false else decr brack
      | _ -> ())
    s;
  !ok && !paren = 0 && !brack = 0

let regex_sane (s : string) : bool = String.length s > 0 && balanced s

(* ── Constraints (TC0010 type-compat, TC0011 well-formedness) ──────────── *)

let check_constraint target span (c : Ir.constraint_) : Diagnostic.t list =
  match c with
  | Ir.Range { min; max; _ } ->
      let ty =
        if is_numeric target then []
        else
          [
            err Error_codes.constraint_type_mismatch span
              "@range applies to numeric types only";
          ]
      in
      let wf =
        match (min, max) with
        | Some a, Some b when a > b ->
            [
              err Error_codes.constraint_malformed span
                "@range has min greater than max";
            ]
        | _ -> []
      in
      ty @ wf
  | Ir.Length { min; max } ->
      let ty =
        if is_lengthy target then []
        else
          [
            err Error_codes.constraint_type_mismatch span
              "@length applies to strings, bytes, lists, and maps only";
          ]
      in
      let negative o = match o with Some n -> n < 0 | None -> false in
      let inverted =
        match (min, max) with Some a, Some b -> a > b | _ -> false
      in
      let wf =
        if negative min || negative max || inverted then
          [
            err Error_codes.constraint_malformed span
              "@length bounds must be non-negative with min not exceeding max";
          ]
        else []
      in
      ty @ wf
  | Ir.Pattern s ->
      let ty =
        if is_string target then []
        else
          [
            err Error_codes.constraint_type_mismatch span
              "@pattern applies to strings only";
          ]
      in
      let wf =
        if regex_sane s then []
        else
          [
            err Error_codes.constraint_malformed span
              "@pattern is not a well-formed regular expression";
          ]
      in
      ty @ wf
  | Ir.MultipleOf f ->
      let ty =
        if is_numeric target then []
        else
          [
            err Error_codes.constraint_type_mismatch span
              "@multipleOf applies to numeric types only";
          ]
      in
      let wf =
        if f <= 0. then
          [
            err Error_codes.constraint_malformed span
              "@multipleOf must be greater than zero";
          ]
        else []
      in
      ty @ wf
  | Ir.Custom _ ->
      [] (* unreachable: lowering routes custom traits to the bag *)

(* ── Defaults (TC0012 type, TC0013 constraint satisfaction) ────────────── *)

(* Lowering builds a default from [json_of_arg], which only yields [`Int]/[`Float]
   for numbers (an out-of-range literal fails earlier), so [`Intlit] never reaches
   here and is not matched. *)
let is_int_json = function `Int _ -> true | _ -> false
let is_num_json = function `Int _ | `Float _ -> true | _ -> false

(* v1 scope: numeric and string scalars are type-checked; other targets (bool,
   temporal, uuid, enums/refs, collections) accept any default for now. *)
let default_type_ok target (j : Ir.json) : bool =
  match target with
  | Ir.Prim (Ir.Int _) -> is_int_json j
  | Ir.Prim Ir.Float -> is_num_json j
  | Ir.Prim Ir.String -> ( match j with `String _ -> true | _ -> false)
  | _ -> true

let num_of_json = function
  | `Int n -> Some (float_of_int n)
  | `Float f -> Some f
  | _ -> None

let default_violations span constraints (j : Ir.json) : Diagnostic.t list =
  let num = num_of_json j in
  let len = match j with `String s -> Some (String.length s) | _ -> None in
  List.concat_map
    (fun (c : Ir.constraint_) ->
      match c with
      (* Exclusive bounds are not expressible on the surface yet (lowering always
         produces inclusive bounds), so they are treated as inclusive here. *)
      | Ir.Range { min; max; _ } -> (
          match num with
          | Some v ->
              let below = match min with Some m -> v < m | None -> false in
              let above = match max with Some m -> v > m | None -> false in
              if below || above then
                [
                  err Error_codes.default_violates_constraint span
                    "default value is outside the @range bounds";
                ]
              else []
          | None -> [])
      | Ir.MultipleOf f -> (
          match num with
          | Some v when f <> 0. && Float.rem v f <> 0. ->
              [
                err Error_codes.default_violates_constraint span
                  "default value is not a multiple of the @multipleOf factor";
              ]
          | _ -> [])
      | Ir.Length { min; max } -> (
          match len with
          | Some l ->
              let below = match min with Some m -> l < m | None -> false in
              let above = match max with Some m -> l > m | None -> false in
              if below || above then
                [
                  err Error_codes.default_violates_constraint span
                    "default value length is outside the @length bounds";
                ]
              else []
          | None -> [])
      (* Matching a default against a @pattern needs a regex engine; deferred. *)
      | Ir.Pattern _ | Ir.Custom _ -> [])
    constraints

let check_default span target constraints (default : Ir.json option) :
    Diagnostic.t list =
  match default with
  | None -> []
  | Some j ->
      if not (default_type_ok target j) then
        [
          err Error_codes.default_type_mismatch span
            "default value does not match the member's type";
        ]
      else default_violations span constraints j

(* ── Per-member and per-module orchestration ───────────────────────────── *)

let check_member span (m : Ir.member) : Diagnostic.t list =
  List.concat_map (check_constraint m.target span) m.constraints
  @ check_default span m.target m.constraints m.default

(* Surface struct members, in declaration order, so an IR member can borrow its
   name span. *)
let struct_members (file : Ast.file) : (string * Ast.member list) list =
  List.filter_map
    (fun (d : Ast.decl) ->
      match d.dkind with
      | Ast.DStruct { members; _ } -> Some (d.dname, members)
      | _ -> None)
    file

let check ~(file : Ast.file) (m : Ir.module_) : Diagnostic.t list =
  let ast = struct_members file in
  List.concat_map
    (fun (sh : Ir.shape) ->
      match sh.kind with
      | Ir.Structure { members = ir_members; _ } -> (
          match List.assoc_opt sh.id ast with
          | Some ast_members
            when List.length ast_members = List.length ir_members ->
              List.concat_map
                (fun ((am : Ast.member), im) -> check_member am.mname_span im)
                (List.combine ast_members ir_members)
          (* Defensive: lowering keeps a struct's IR members aligned 1:1 with its
             surface members, so a missing entry or length mismatch cannot occur
             for a well-formed pipeline; without spans there is nothing to do. *)
          | _ -> [])
      | _ -> [])
    m.shapes
