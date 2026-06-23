(* The calculus type checker. It validates a program (a set of named functions)
   against the IR module that supplies the nominal types its expressions
   reference -- struct fields, union variants, enum values. It produces stable
   [CAxxxx] diagnostics, never raises, and accumulates every finding.

   Three pieces make the language total, and they live here: division and modulo
   demand a static proof the divisor is non-zero (tracked syntactically in
   [ctx.nonzero], populated by an [if x != 0] guard); every [match] on an (always
   open) sum must cover all variants plus a mandatory [Unknown] arm; and the
   function call graph must be acyclic (no recursion). *)

module A = Calc_ast
module Ty = Calc_types
module Codes = Calc_error_codes

type st = {
  module_ : Ir.module_;
  fns : (string * Ty.t) list; (* function name -> its Fn type *)
  mutable diags : Diagnostic.t list;
}

(* A typing context: locals in scope and the idents proven non-zero on the
   current branch. *)
type ctx = { vars : (string * Ty.t) list; nonzero : string list }

let err st span code fmt =
  Printf.ksprintf
    (fun message ->
      st.diags <-
        {
          Diagnostic.span;
          severity = Diagnostic.Error;
          message;
          code = Some code;
        }
        :: st.diags)
    fmt

(* ── IR lookups ────────────────────────────────────────────────────────── *)

let rec of_tref (t : Ir.tref) : Ty.t =
  match t with
  | Ir.Prim p -> Ty.Prim p
  | Ir.Ref (id, args) -> Ty.Ref (id, List.map of_tref args)
  | Ir.Param s -> Ty.Ref (s, []) (* an opaque type parameter *)
  | Ir.List t -> Ty.List (of_tref t)
  | Ir.Map (k, v) -> Ty.Map (of_tref k, of_tref v)

let find_shape st id =
  List.find_opt (fun (s : Ir.shape) -> String.equal s.id id) st.module_.shapes

(* A struct's fields as (name, type), or None if [id] is not a struct. *)
let struct_fields st id =
  match find_shape st id with
  | Some { kind = Ir.Structure { members; _ }; _ } ->
      Some
        (List.map (fun (m : Ir.member) -> (m.name, of_tref m.target)) members)
  | _ -> None

(* A sum's variants as (name, payload type option), or None if [id] is not a
   union or enum. Union variants carry a payload; enum values do not. *)
let sum_variants st id =
  match find_shape st id with
  | Some { kind = Ir.Union { members; _ }; _ } ->
      Some
        (List.map
           (fun (m : Ir.member) -> (m.name, Some (of_tref m.target)))
           members)
  | Some { kind = Ir.Enum { values; _ }; _ } ->
      Some (List.map (fun (name, _) -> (name, None)) values)
  | _ -> None

(* ── Type inference ────────────────────────────────────────────────────── *)

(* The numeric result of an arithmetic operator, or [Err] with a diagnostic.
   Operands must be the same width and signedness (no implicit promotion); an
   integer literal adopts the other operand's width. *)
let num_result st span (ta : Ty.t) (tb : Ty.t) : Ty.t =
  match (ta, tb) with
  | Ty.Err, _ | _, Ty.Err -> Ty.Err
  | Ty.Int_lit, Ty.Int_lit -> Ty.Int_lit
  | Ty.Int_lit, (Ty.Prim (Ir.Int _) as t)
  | (Ty.Prim (Ir.Int _) as t), Ty.Int_lit ->
      t
  | ( Ty.Prim (Ir.Int { bits = b1; signed = s1 }),
      Ty.Prim (Ir.Int { bits = b2; signed = s2 }) )
    when b1 = b2 && s1 = s2 ->
      Ty.Prim (Ir.Int { bits = b1; signed = s1 })
  | Ty.Prim Ir.Float, Ty.Prim Ir.Float -> Ty.Prim Ir.Float
  | _ ->
      err st span Codes.type_mismatch
        "arithmetic needs two numbers of the same type, got %s and %s"
        (Ty.to_string ta) (Ty.to_string tb);
      Ty.Err

(* Whether [b] is statically a non-zero divisor: a non-zero literal, or an ident
   proven non-zero on this branch. *)
let divisor_nonzero ctx (b : A.expr) =
  match b.kind with
  | A.Lit (A.Int s) -> not (String.equal s "0" || String.equal s "-0")
  | A.Var x -> List.mem x ctx.nonzero
  | _ -> false

(* Narrow the then/else contexts of an [if] whose guard is one of the four
   recognized non-zero forms. *)
let narrow ctx (c : A.expr) : ctx * ctx =
  let with_nz x = { ctx with nonzero = x :: ctx.nonzero } in
  let is_zero (e : A.expr) =
    match e.kind with A.Lit (A.Int "0") -> true | _ -> false
  in
  let var (e : A.expr) = match e.kind with A.Var x -> Some x | _ -> None in
  match c.kind with
  | A.Binop (A.Ne, a, b) -> (
      match (var a, is_zero b, var b, is_zero a) with
      | Some x, true, _, _ | _, _, Some x, true -> (with_nz x, ctx)
      | _ -> (ctx, ctx))
  | A.Binop (A.Eq, a, b) -> (
      match (var a, is_zero b, var b, is_zero a) with
      | Some x, true, _, _ | _, _, Some x, true -> (ctx, with_nz x)
      | _ -> (ctx, ctx))
  | _ -> (ctx, ctx)

(* The shared type of two branch results, or [Err] with a diagnostic under
   [code] (match arms report [divergent_arms]; other branches [type_mismatch]). *)
let join ?(code = Codes.type_mismatch) st span (t1 : Ty.t) (t2 : Ty.t) : Ty.t =
  if Ty.compat t1 t2 || Ty.compat t2 t1 then
    match t1 with Ty.Err | Ty.Int_lit -> t2 | _ -> t1
  else (
    err st span code "branches have different types: %s and %s"
      (Ty.to_string t1) (Ty.to_string t2);
    Ty.Err)

let rec infer st ctx (e : A.expr) : Ty.t =
  match e.kind with
  | A.Lit (A.Int _) -> Ty.Int_lit
  | A.Lit (A.Float _) -> Ty.Prim Ir.Float
  | A.Lit (A.Str _) -> Ty.Prim Ir.String
  | A.Lit (A.Bool _) -> Ty.Prim Ir.Bool
  | A.Var x -> (
      match List.assoc_opt x ctx.vars with
      | Some t -> t
      | None ->
          err st e.span Codes.unbound "unbound variable '%s'" x;
          Ty.Err)
  | A.Field (obj, f) -> infer_field st ctx e.span obj f
  | A.Not e1 ->
      expect st ctx (Ty.Prim Ir.Bool) e1 "operand of '!'";
      Ty.Prim Ir.Bool
  | A.Binop (op, a, b) -> infer_binop st ctx e.span op a b
  | A.If (c, t, el) ->
      expect st ctx (Ty.Prim Ir.Bool) c "condition of 'if'";
      let ctx_t, ctx_e = narrow ctx c in
      join st e.span (infer st ctx_t t) (infer st ctx_e el)
  | A.Let (x, v, body) ->
      let tv = infer st ctx v in
      infer st { ctx with vars = (x, tv) :: ctx.vars } body
  | A.Call (f, args) -> infer_call st ctx e.span f args
  | A.Map (l, fn) -> infer_map st ctx e.span ~keep:false l fn
  | A.Filter (l, fn) -> infer_map st ctx e.span ~keep:true l fn
  | A.Fold (l, init, fn) -> infer_fold st ctx e.span l init fn
  | A.Match (scrut, arms) -> infer_match st ctx e.span scrut arms
  | A.Some_ e1 -> Ty.Opt (infer st ctx e1)
  | A.None_ -> Ty.Opt Ty.Err
  | A.Coalesce (a, b) -> infer_coalesce st ctx e.span a b
  | A.Ctor (name, fields) -> infer_ctor st ctx e.span name fields
  | A.EError -> Ty.Err

(* Infer [e] and require it compatible with [want]; otherwise diagnose. *)
and expect st ctx (want : Ty.t) (e : A.expr) what =
  let got = infer st ctx e in
  if not (Ty.compat got want) then
    err st e.span Codes.type_mismatch "%s must be %s, got %s" what
      (Ty.to_string want) (Ty.to_string got)

and infer_field st ctx span obj f =
  let t = infer st ctx obj in
  match t with
  | Ty.Err -> Ty.Err
  | Ty.Ref (id, _) -> (
      match struct_fields st id with
      | None ->
          err st span Codes.field_access "'%s' is not a struct" (Ty.to_string t);
          Ty.Err
      | Some fields -> (
          match List.assoc_opt f fields with
          | Some ft -> ft
          | None ->
              err st span Codes.field_access "%s has no field '%s'"
                (Ty.to_string t) f;
              Ty.Err))
  | _ ->
      err st span Codes.field_access "'%s' is not a struct" (Ty.to_string t);
      Ty.Err

and infer_binop st ctx span op a b =
  let ta = infer st ctx a and tb = infer st ctx b in
  match op with
  | A.Add | A.Sub | A.Mul -> num_result st span ta tb
  | A.Div | A.Mod ->
      let r = num_result st span ta tb in
      (* Only integer division/modulo can trap; float division is IEEE. *)
      if Ty.is_int r && not (divisor_nonzero ctx b) then
        err st span Codes.unguarded_division
          "'%s' requires a proof the divisor is non-zero (guard with 'if d != \
           0')"
          (match op with A.Div -> "/" | _ -> "%");
      r
  | A.Eq | A.Ne ->
      if not (Ty.compat ta tb || Ty.compat tb ta) then
        err st span Codes.type_mismatch "cannot compare %s with %s"
          (Ty.to_string ta) (Ty.to_string tb);
      Ty.Prim Ir.Bool
  | A.Lt | A.Gt | A.Le | A.Ge ->
      ignore (num_result st span ta tb);
      Ty.Prim Ir.Bool
  | A.And | A.Or ->
      if not (Ty.compat ta (Ty.Prim Ir.Bool)) then
        err st span Codes.type_mismatch
          "operand of a logical operator must be bool, got %s" (Ty.to_string ta);
      if not (Ty.compat tb (Ty.Prim Ir.Bool)) then
        err st span Codes.type_mismatch
          "operand of a logical operator must be bool, got %s" (Ty.to_string tb);
      Ty.Prim Ir.Bool
  | A.Concat ->
      if not (Ty.compat ta (Ty.Prim Ir.String)) then
        err st span Codes.type_mismatch "'++' joins strings, got %s"
          (Ty.to_string ta);
      if not (Ty.compat tb (Ty.Prim Ir.String)) then
        err st span Codes.type_mismatch "'++' joins strings, got %s"
          (Ty.to_string tb);
      Ty.Prim Ir.String

(* A named function referenced by a combinator or [find]. *)
and lookup_fn st span name : (Ty.t list * Ty.t) option =
  match List.assoc_opt name st.fns with
  | Some (Ty.Fn (ps, r)) -> Some (ps, r)
  | _ ->
      err st span Codes.unbound "unknown function '%s'" name;
      None

and infer_call st ctx span f args =
  match builtin st ctx span f args with
  | Some t -> t
  | None -> (
      match List.assoc_opt f st.fns with
      | Some (Ty.Fn (params, ret)) ->
          check_args st ctx span f params args;
          ret
      | _ ->
          err st span Codes.unbound "unknown function '%s'" f;
          List.iter (fun a -> ignore (infer st ctx a)) args;
          Ty.Err)

(* Check positional arguments against expected parameter types. *)
and check_args st ctx span what params args =
  if List.length params <> List.length args then
    err st span Codes.wrong_arity "%s expects %d argument(s), got %d" what
      (List.length params) (List.length args)
  else List.iter2 (fun p a -> expect st ctx p a "argument") params args

(* Type the closed builtin set. Returns None when [name] is not a builtin. *)
and builtin st ctx span name args : Ty.t option =
  let one () = match args with [ a ] -> Some (infer st ctx a) | _ -> None in
  match (name, args) with
  | "length", [ a ] -> Some (list_elem st ctx a |> fun _ -> Ty.i64)
  | "head", [ a ] -> Some (Ty.Opt (list_elem st ctx a))
  | "get", [ a; i ] ->
      let t = list_elem st ctx a in
      expect st ctx Ty.i64 i "list index";
      Some (Ty.Opt t)
  | "find", [ a; fn ] ->
      let t = list_elem st ctx a in
      check_pred st span t fn;
      Some (Ty.Opt t)
  | "lookup", [ m; k ] ->
      let kt, vt = map_kv st ctx m in
      expect st ctx kt k "map key";
      Some (Ty.Opt vt)
  | "get_or", [ m; k; d ] ->
      let kt, vt = map_kv st ctx m in
      expect st ctx kt k "map key";
      expect st ctx vt d "default value";
      Some vt
  | "to_int", [ a ] ->
      expect st ctx (Ty.Prim Ir.Float) a "argument of to_int";
      Some (Ty.Opt Ty.i64)
  | "to_float", [ a ] ->
      let t = infer st ctx a in
      if not (Ty.is_int t) then
        err st span Codes.type_mismatch "to_float expects an integer, got %s"
          (Ty.to_string t);
      Some (Ty.Prim Ir.Float)
  | ( ( "length" | "head" | "get" | "find" | "lookup" | "get_or" | "to_int"
      | "to_float" ),
      _ ) ->
      (* a known builtin applied with the wrong number of arguments *)
      ignore (one ());
      List.iter (fun a -> ignore (infer st ctx a)) args;
      err st span Codes.wrong_arity "'%s' got the wrong number of arguments"
        name;
      Some Ty.Err
  | _ -> None

(* The element type of a list argument, diagnosing a non-list. *)
and list_elem st ctx (a : A.expr) : Ty.t =
  match infer st ctx a with
  | Ty.List t -> t
  | Ty.Err -> Ty.Err
  | other ->
      err st a.span Codes.type_mismatch "expected a list, got %s"
        (Ty.to_string other);
      Ty.Err

(* The key/value types of a map argument, diagnosing a non-map. *)
and map_kv st ctx (a : A.expr) : Ty.t * Ty.t =
  match infer st ctx a with
  | Ty.Map (k, v) -> (k, v)
  | Ty.Err -> (Ty.Err, Ty.Err)
  | other ->
      err st a.span Codes.type_mismatch "expected a map, got %s"
        (Ty.to_string other);
      (Ty.Err, Ty.Err)

(* A predicate named-function argument: must be [elem -> bool]. *)
and check_pred st span (elem : Ty.t) (fn : A.expr) =
  match fn.kind with
  | A.Var name -> (
      match lookup_fn st span name with
      | Some ([ p ], r) ->
          if not (Ty.compat elem p && Ty.compat r (Ty.Prim Ir.Bool)) then
            err st span Codes.type_mismatch "predicate must be (%s) -> bool"
              (Ty.to_string elem)
      | Some _ ->
          err st span Codes.type_mismatch "predicate must take one argument"
      | None -> ())
  | _ -> err st fn.span Codes.type_mismatch "expected a function name"

and infer_map st ctx span ~keep l fn =
  let elem = list_elem st ctx l in
  match lookup_fn st span fn with
  | None -> Ty.List Ty.Err
  | Some ([ p ], r) ->
      if not (Ty.compat elem p) then
        err st span Codes.type_mismatch
          "function expects %s but the list holds %s" (Ty.to_string p)
          (Ty.to_string elem);
      if keep then (
        if not (Ty.compat r (Ty.Prim Ir.Bool)) then
          err st span Codes.type_mismatch "filter needs a (%s) -> bool function"
            (Ty.to_string elem);
        Ty.List elem)
      else Ty.List r
  | Some _ ->
      err st span Codes.type_mismatch
        "combinator function must take one argument";
      Ty.List Ty.Err

and infer_fold st ctx span l init fn =
  let elem = list_elem st ctx l in
  let acc = infer st ctx init in
  match lookup_fn st span fn with
  | None -> acc
  | Some ([ pacc; pelem ], r) ->
      if not (Ty.compat acc pacc && Ty.compat elem pelem && Ty.compat r pacc)
      then
        err st span Codes.type_mismatch "fold needs a (%s, %s) -> %s function"
          (Ty.to_string acc) (Ty.to_string elem) (Ty.to_string acc);
      acc
  | Some _ ->
      err st span Codes.type_mismatch "fold function must take two arguments";
      acc

and infer_coalesce st ctx span a b =
  let ta = infer st ctx a in
  let tb = infer st ctx b in
  match ta with
  | Ty.Err -> tb
  | Ty.Opt inner ->
      if Ty.compat inner tb || Ty.compat tb inner then join st span inner tb
      else (
        err st span Codes.type_mismatch
          "'??' default is %s but the value holds %s" (Ty.to_string tb)
          (Ty.to_string inner);
        Ty.Err)
  | other ->
      err st span Codes.type_mismatch
        "'??' left operand must be optional, got %s" (Ty.to_string other);
      tb

and infer_ctor st ctx span name fields =
  match struct_fields st name with
  | None ->
      err st span Codes.type_mismatch "'%s' is not a struct" name;
      List.iter (fun (_, e) -> ignore (infer st ctx e)) fields;
      Ty.Err
  | Some decl ->
      List.iter
        (fun (fname, e) ->
          match List.assoc_opt fname decl with
          | Some ft -> expect st ctx ft e (Printf.sprintf "field '%s'" fname)
          | None ->
              err st span Codes.field_access "struct '%s' has no field '%s'"
                name fname)
        fields;
      Ty.Ref (name, [])

and infer_match st ctx span scrut arms =
  let ts = infer st ctx scrut in
  match ts with
  | Ty.Err -> Ty.Err
  | Ty.Ref (id, _) -> (
      match sum_variants st id with
      | Some variants -> check_match st ctx span id variants arms
      | None ->
          err st span Codes.match_subject "match needs a union or enum, got %s"
            (Ty.to_string ts);
          Ty.Err)
  | _ ->
      err st span Codes.match_subject "match needs a union or enum, got %s"
        (Ty.to_string ts);
      Ty.Err

and check_match st ctx span id variants arms =
  let covered = ref [] in
  let has_unknown = ref false in
  let results =
    List.map
      (fun (p, body) ->
        match p with
        | A.PUnknown { bind } ->
            has_unknown := true;
            infer st
              { ctx with vars = (bind, Ty.Prim Ir.Bytes) :: ctx.vars }
              body
        | A.PVariant { variant; bind } -> (
            covered := variant :: !covered;
            match List.assoc_opt variant variants with
            | None ->
                err st span Codes.unknown_variant
                  "'%s' is not a variant of '%s'" variant id;
                infer st ctx body
            | Some payload ->
                let vars =
                  match (bind, payload) with
                  | Some b, Some t -> (b, t) :: ctx.vars
                  | Some b, None -> (b, Ty.Err) :: ctx.vars
                  | None, _ -> ctx.vars
                in
                infer st { ctx with vars } body))
      arms
  in
  let missing =
    List.filter (fun (v, _) -> not (List.mem v !covered)) variants
  in
  if missing <> [] then
    err st span Codes.non_exhaustive "match is missing variant(s): %s"
      (String.concat ", " (List.map fst missing));
  if not !has_unknown then
    err st span Codes.non_exhaustive
      "match on the open sum '%s' needs an 'Unknown' arm" id;
  match results with
  | [] -> Ty.Err
  | first :: rest ->
      List.fold_left
        (fun acc t -> join ~code:Codes.divergent_arms st span acc t)
        first rest

(* ── Call graph (recursion is forbidden) ───────────────────────────────── *)

(* The names this function references that are themselves functions. *)
let callees st (fd : A.fn_def) : string list =
  let acc = ref [] in
  let add n = if List.mem_assoc n st.fns then acc := n :: !acc in
  let rec walk (e : A.expr) =
    match e.kind with
    | A.Call (f, args) ->
        add f;
        List.iter walk args
    | A.Map (l, fn) | A.Filter (l, fn) ->
        add fn;
        walk l
    | A.Fold (l, i, fn) ->
        add fn;
        walk l;
        walk i
    | A.Field (e, _) | A.Not e | A.Some_ e -> walk e
    | A.Binop (_, a, b) | A.Coalesce (a, b) ->
        walk a;
        walk b
    | A.If (a, b, c) ->
        walk a;
        walk b;
        walk c
    | A.Let (_, v, b) ->
        walk v;
        walk b
    | A.Match (s, arms) ->
        walk s;
        List.iter (fun (_, b) -> walk b) arms
    | A.Ctor (_, fields) -> List.iter (fun (_, e) -> walk e) fields
    | A.Lit _ | A.Var _ | A.None_ | A.EError -> ()
  in
  walk fd.body;
  !acc

let check_acyclic st prog =
  let bodies = List.map (fun (fd : A.fn_def) -> (fd.name, fd)) prog in
  let state = Hashtbl.create 16 in
  let rec visit name span =
    match Hashtbl.find_opt state name with
    | Some `Gray ->
        err st span Codes.recursive_fn
          "'%s' is part of a recursive cycle, which the calculus forbids" name
    | Some `Black -> ()
    | _ ->
        Hashtbl.replace state name `Gray;
        (match List.assoc_opt name bodies with
        | Some fd -> List.iter (fun c -> visit c fd.name_span) (callees st fd)
        | None -> ());
        Hashtbl.replace state name `Black
  in
  List.iter (fun (fd : A.fn_def) -> visit fd.name fd.name_span) prog

(* ── Entry point ───────────────────────────────────────────────────────── *)

let check (m : Ir.module_) (prog : A.program) : Diagnostic.t list =
  let fns =
    List.map
      (fun (fd : A.fn_def) ->
        let params = List.map (fun (_, ty) -> Ty.resolve ty) fd.params in
        (fd.name, Ty.Fn (params, Ty.resolve fd.ret)))
      prog
  in
  let st = { module_ = m; fns; diags = [] } in
  check_acyclic st prog;
  List.iter
    (fun (fd : A.fn_def) ->
      let vars = List.map (fun (n, ty) -> (n, Ty.resolve ty)) fd.params in
      let tbody = infer st { vars; nonzero = [] } fd.body in
      let tret = Ty.resolve fd.ret in
      if not (Ty.compat tbody tret) then
        err st fd.name_span Codes.type_mismatch
          "'%s' is declared to return %s but its body has type %s" fd.name
          (Ty.to_string tret) (Ty.to_string tbody))
    prog;
  Diagnostic.sort (List.rev st.diags)
