(* Typed lowering of test-block values to their wire form. The test checker
   drives this against the declared types (entry fields, op inputs/outputs,
   declared errors); the encoding rules are the wire contract: i64/u64 as
   strings, an omitted optional member absent, a plain value filling an
   optional without any Some wrapper, and dataflow references as
   {"$ref":{"binding":...,"path":[...]}} leaves. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt
let dummy_pos : Span.pos = { line = 0; col = 0; offset = 0 }
let dummy_span : Span.span = { start = dummy_pos; finish = dummy_pos }

(* The two language modules a test may import. Their shapes exist only inside
   test blocks; they never become IR modules. *)
type virtual_mod = Vhttp | Verrors

type ctx = {
  decls : Ast.decl list;
  virtuals : (string * virtual_mod) list; (* import qualifier -> module *)
  diags : Diagnostic.t list ref;
}

let report ctx d = ctx.diags := d :: !(ctx.diags)

let virtual_of_import (i : Ast.import) : virtual_mod option =
  match i.Ast.imported_path with
  | [ "tono"; "http" ] -> Some Vhttp
  | [ "tono"; "errors" ] -> Some Verrors
  | _ -> None

let make_ctx ~(imports : Ast.import list) ~(decls : Ast.decl list)
    ~(diags : Diagnostic.t list ref) : ctx =
  let virtuals =
    List.filter_map
      (fun (i : Ast.import) ->
        Option.map
          (fun vm -> (Modules.qualifier_of i, vm))
          (virtual_of_import i))
      imports
  in
  { decls; virtuals; diags }

let decl_by_name ctx name =
  List.find_opt (fun (d : Ast.decl) -> String.equal d.Ast.dname name) ctx.decls

let struct_members ctx name : Ast.member list option =
  match decl_by_name ctx name with
  | Some { dkind = Ast.DStruct { members; _ }; _ } -> Some members
  | _ -> None

let base_ty : Ast.ty -> Ast.ty = function Ast.TNullable (t, _) -> t | t -> t

let find_trait name (traits : Ast.trait list) : Ast.trait option =
  List.find_opt (fun (t : Ast.trait) -> String.equal t.Ast.tname name) traits

(* Whether a value constructor may omit this member: a nullable member is
   simply absent, and a defaulted one falls back to its declared default. *)
let member_optional (m : Ast.member) : bool =
  (match m.Ast.mtype with Ast.TNullable _ -> true | _ -> false)
  || Option.is_some (find_trait "default" m.Ast.mtraits)

(* ── Constructor heads ─────────────────────────────────────────────────── *)

(* What a ctor head resolves to: a module-local shape, a language-module shape
   ([http.response], [errors.api]), or an already-diagnosed failure. A bare
   [http.x]/[errors.x] head without the matching import is the one place the
   missing import is suggested by name. *)
type head_kind = Huser of string | Hvirtual of virtual_mod * string | Hbad

let resolve_head ctx ~(code : string) (h : Ast.value_head) : head_kind =
  match h.Ast.vh_segs with
  | [ n ] -> Huser n
  | [ q; n ] -> (
      match List.assoc_opt q ctx.virtuals with
      | Some vm -> Hvirtual (vm, n)
      | None ->
          let import_hint root =
            report ctx
              (err Error_codes.test_import_missing h.vh_span
                 "'%s.%s' comes from the language module tono.%s; add 'import \
                  tono.%s' at the top of the file"
                 q n root root)
          in
          (match q with
          | "http" -> import_hint "http"
          | "errors" -> import_hint "errors"
          | _ ->
              report ctx (err code h.vh_span "unknown constructor '%s.%s'" q n));
          Hbad)
  | segs ->
      report ctx
        (err code h.vh_span "'%s' is not a constructor name"
           (String.concat "." segs));
      Hbad

(* ── Value encoding ────────────────────────────────────────────────────── *)

let value_span : Ast.test_value -> Span.span = function
  | Ast.TvStr (_, s)
  | Ast.TvInt (_, s)
  | Ast.TvFloat (_, s)
  | Ast.TvBool (_, s)
  | Ast.TvCtor { tc_span = s; _ }
  | Ast.TvList (_, s)
  | Ast.TvMap (_, s)
  | Ast.TvRef { ref_span = s; _ }
  | Ast.TvError s ->
      s

(* Types a dataflow reference ([saved.id]) against the bindings in scope; the
   checker supplies it (and reports its own diagnostics). [None] means the
   reference did not type; the leaf is still emitted so one bad reference does
   not cascade. *)
type ref_typer = base:string -> path:string list -> Span.span -> Ast.ty option

let ref_leaf base path : Ir.json =
  `Assoc
    [
      ( "$ref",
        `Assoc
          [
            ("binding", `String base);
            ("path", `List (List.map (fun s -> `String s) path));
          ] );
    ]

let describe_value : Ast.test_value -> string = function
  | Ast.TvStr _ -> "a string"
  | Ast.TvInt _ -> "an integer"
  | Ast.TvFloat _ -> "a number"
  | Ast.TvBool _ -> "a boolean"
  | Ast.TvCtor _ -> "a constructor"
  | Ast.TvList _ -> "a list"
  | Ast.TvMap _ -> "a map"
  | Ast.TvRef _ -> "a reference"
  | Ast.TvError _ -> "an invalid value"

let string_prims =
  [ "string"; "bytes"; "timestamp"; "date"; "duration"; "uuid" ]

let mismatch ctx span expected (v : Ast.test_value) : Ir.json =
  report ctx
    (err Error_codes.test_value_invalid span "expected %s, found %s" expected
       (describe_value v));
  `Null

let rec encode_value ctx ~(refty : ref_typer) (expected : Ast.ty)
    (v : Ast.test_value) : Ir.json =
  match v with
  | Ast.TvError _ -> `Null (* the parser already diagnosed *)
  | Ast.TvRef { base; path; ref_span } ->
      ignore (refty ~base ~path ref_span);
      ref_leaf base path
  | _ -> (
      match base_ty expected with
      | Ast.TError _ -> `Null
      | Ast.TPrim (p, _) -> encode_prim ctx p v
      | Ast.TList (t, _) -> (
          match v with
          | Ast.TvList (items, _) ->
              `List (List.map (encode_value ctx ~refty t) items)
          | _ -> mismatch ctx (value_span v) "a list" v)
      | Ast.TMap (_, vt, _) -> (
          match v with
          | Ast.TvMap (entries, _) ->
              `Assoc
                (List.map
                   (fun ((k, _), ev) -> (k, encode_value ctx ~refty vt ev))
                   entries)
          | _ -> mismatch ctx (value_span v) "a map" v)
      | Ast.TName (n, [], _) -> encode_named ctx ~refty n v
      | Ast.TName _ | Ast.TQName _ ->
          report ctx
            (err Error_codes.test_value_invalid (value_span v)
               "generic and cross-module types are not supported in test values");
          `Null
      | Ast.TNullable _ -> assert false (* stripped by [base_ty] *))

and encode_prim ctx (p : string) (v : Ast.test_value) : Ir.json =
  match (p, v) with
  | p', Ast.TvStr (s, _) when List.mem p' string_prims -> `String s
  | "bool", Ast.TvBool (b, _) -> `Bool b
  | "float", Ast.TvFloat (f, _) -> `Float f
  | "float", Ast.TvInt (n, _) -> `Float (float_of_int n)
  | ("i64" | "u64"), Ast.TvInt (n, _) ->
      (* Wire form: 64-bit integers travel as strings. *)
      `String (string_of_int n)
  | ("i8" | "i16" | "i32" | "u8" | "u16" | "u32"), Ast.TvInt (n, _) -> `Int n
  | _ -> mismatch ctx (value_span v) (Printf.sprintf "a %s value" p) v

and encode_named ctx ~refty (n : string) (v : Ast.test_value) : Ir.json =
  match decl_by_name ctx n with
  | Some { dkind = Ast.DStruct { members; _ }; _ } -> (
      match v with
      | Ast.TvCtor c -> (
          match
            resolve_head ctx ~code:Error_codes.test_value_invalid c.tc_head
          with
          | Huser head when String.equal head n ->
              encode_ctor_fields ctx ~refty ~shape:n ~members c
          | Huser head ->
              report ctx
                (err Error_codes.test_value_invalid c.tc_head.vh_span
                   "expected a '%s' constructor, found '%s'" n head);
              `Null
          | Hvirtual _ ->
              report ctx
                (err Error_codes.test_value_invalid c.tc_head.vh_span
                   "expected a '%s' constructor" n);
              `Null
          | Hbad -> `Null)
      | _ -> mismatch ctx (value_span v) (Printf.sprintf "a '%s' value" n) v)
  | Some { dkind = Ast.DEnum { cases }; _ } -> (
      match v with
      | Ast.TvStr (s, span) -> (
          match
            List.find_opt
              (fun (c : Ast.enum_case) -> String.equal c.Ast.cname s)
              cases
          with
          | None ->
              report ctx
                (err Error_codes.test_value_invalid span
                   "'%s' is not a value of enum '%s'" s n);
              `Null
          | Some case ->
              let int_backed =
                List.exists
                  (fun (c : Ast.enum_case) -> c.Ast.cint <> None)
                  cases
              in
              if int_backed then
                match case.Ast.cint with Some i -> `Int i | None -> `String s
              else `String s)
      | _ ->
          mismatch ctx (value_span v)
            (Printf.sprintf "a '%s' enum value (a string)" n)
            v)
  | Some _ ->
      report ctx
        (err Error_codes.test_value_invalid (value_span v)
           "'%s' values cannot be written in a test" n);
      `Null
  | None ->
      (* An unresolved type was already reported by the resolve pass. *)
      `Null

and encode_ctor_fields ctx ~refty ~(shape : string) ~(members : Ast.member list)
    (c : Ast.test_ctor) : Ir.json =
  let field_member name =
    List.find_opt
      (fun (m : Ast.member) -> String.equal m.Ast.mname name)
      members
  in
  let encoded =
    List.filter_map
      (fun (name, name_span, fv) ->
        match field_member name with
        | None ->
            report ctx
              (err Error_codes.test_value_invalid name_span
                 "'%s' has no field '%s'" shape name);
            None
        | Some m -> Some (name, encode_value ctx ~refty m.Ast.mtype fv))
      c.Ast.tc_fields
  in
  (* A required member with no value has no wire form to take. *)
  List.iter
    (fun (m : Ast.member) ->
      if
        (not (member_optional m))
        && not
             (List.exists
                (fun (n, _, _) -> String.equal n m.Ast.mname)
                c.Ast.tc_fields)
      then
        report ctx
          (err Error_codes.test_value_invalid c.Ast.tc_span
             "'%s' requires field '%s'" shape m.Ast.mname))
    members;
  `Assoc encoded
