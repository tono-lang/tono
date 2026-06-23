(* The reference evaluator: the calculus's own evaluation is the semantic
   reference the five target languages must reproduce bit-for-bit. It is total --
   it never throws and always returns a value -- which is exactly the property a
   well-typed program guarantees.

   The load-bearing part is the deterministic integer semantics: arithmetic wraps
   (two's complement at the type's width for signed, modulo 2^w for unsigned) and
   division/modulo truncate toward zero with the remainder taking the sign of the
   dividend. Values are normalized to their width on every operation. *)

(* ── Numeric semantics (the cross-language contract, in OCaml) ─────────── *)

module Num = struct
  (* Normalize a 64-bit pattern to [bits] width, sign-extending when signed. *)
  let wrap ~bits ~signed (v : int64) : int64 =
    if bits >= 64 then v
    else
      let mask = Int64.sub (Int64.shift_left 1L bits) 1L in
      let m = Int64.logand v mask in
      if signed && Int64.logand m (Int64.shift_left 1L (bits - 1)) <> 0L then
        Int64.logor m (Int64.lognot mask) (* sign-extend *)
      else m

  let add ~bits ~signed a b = wrap ~bits ~signed (Int64.add a b)
  let sub ~bits ~signed a b = wrap ~bits ~signed (Int64.sub a b)
  let mul ~bits ~signed a b = wrap ~bits ~signed (Int64.mul a b)

  (* Truncated division/modulo. [INT_MIN / -1] is defined to wrap rather than
     trap, so the only obligation the type checker imposes is a non-zero divisor;
     a zero divisor cannot reach here in a well-typed program, so it is treated
     defensively as a no-op. *)
  let div ~bits ~signed a b =
    if Int64.equal b 0L then 0L
    else if signed then wrap ~bits ~signed (Int64.div a b)
    else wrap ~bits ~signed (Int64.unsigned_div a b)

  let rem ~bits ~signed a b =
    if Int64.equal b 0L then 0L
    else if signed then wrap ~bits ~signed (Int64.rem a b)
    else wrap ~bits ~signed (Int64.unsigned_rem a b)

  (* float -> int? : truncate toward zero; non-finite has no integer form. *)
  let to_int (f : float) : int64 option =
    if Float.is_finite f then Some (Int64.of_float (Float.trunc f)) else None

  (* int -> float : total, lossy above 2^53. *)
  let to_float ~signed (v : int64) : float =
    if signed then Int64.to_float v
    else if Int64.compare v 0L >= 0 then Int64.to_float v
    else
      (* interpret the bit pattern as unsigned *)
      (Int64.to_float (Int64.shift_right_logical v 1) *. 2.0)
      +. Int64.to_float (Int64.logand v 1L)
end

(* ── Values and program evaluation ─────────────────────────────────────── *)

module A = Calc_ast

(* A runtime value. Integers carry their width and signedness so arithmetic can
   wrap correctly; an integer literal defaults to i64. *)
type value =
  | VInt of int64 * int * bool
  | VFloat of float
  | VStr of string
  | VBool of bool
  | VList of value list
  | VMap of (value * value) list
  | VStruct of (string * value) list
  | VVariant of string * value
  | VOpt of value option

(* Raised only on an ill-typed program, which the type checker rules out; a
   well-typed program evaluates totally without reaching this. *)
exception Stuck of string

let vint n = VInt (Int64.of_int n, 64, true)

let rec eval (prog : A.program) env (e : A.expr) : value =
  match e.kind with
  | A.Lit (A.Int s) -> VInt (Int64.of_string s, 64, true)
  | A.Lit (A.Float f) -> VFloat f
  | A.Lit (A.Str s) -> VStr s
  | A.Lit (A.Bool b) -> VBool b
  | A.Var x -> ( try List.assoc x env with Not_found -> raise (Stuck x))
  | A.Field (o, f) -> (
      match eval prog env o with
      | VStruct fields -> List.assoc f fields
      | _ -> raise (Stuck "field of non-struct"))
  | A.Not e1 -> (
      match eval prog env e1 with
      | VBool b -> VBool (not b)
      | _ -> raise (Stuck "not"))
  | A.Binop (op, a, b) -> eval_binop prog env op a b
  | A.If (c, t, el) -> (
      match eval prog env c with
      | VBool true -> eval prog env t
      | VBool false -> eval prog env el
      | _ -> raise (Stuck "if"))
  | A.Let (x, v, body) -> eval prog ((x, eval prog env v) :: env) body
  | A.Call (f, args) -> eval_call prog env f args
  | A.Map (l, fn) ->
      VList (List.map (fun v -> apply prog fn [ v ]) (as_list prog env l))
  | A.Filter (l, fn) ->
      VList
        (List.filter
           (fun v -> match apply prog fn [ v ] with VBool b -> b | _ -> false)
           (as_list prog env l))
  | A.Fold (l, init, fn) ->
      List.fold_left
        (fun acc v -> apply prog fn [ acc; v ])
        (eval prog env init) (as_list prog env l)
  | A.Match (s, arms) -> eval_match prog env (eval prog env s) arms
  | A.Some_ e1 -> VOpt (Some (eval prog env e1))
  | A.None_ -> VOpt None
  | A.Coalesce (a, b) -> (
      match eval prog env a with
      | VOpt (Some v) -> v
      | VOpt None -> eval prog env b
      | _ -> raise (Stuck "coalesce"))
  | A.Ctor (n, fields) -> (
      (* A bare name applied to one positional field is a union variant; a record
         literal is a struct value. *)
      match fields with
      | [ ("", payload) ] -> VVariant (n, eval prog env payload)
      | _ -> VStruct (List.map (fun (f, e) -> (f, eval prog env e)) fields))
  | A.EError -> raise (Stuck "parse error")

and as_list prog env e =
  match eval prog env e with
  | VList l -> l
  | _ -> raise (Stuck "expected a list")

and eval_binop prog env op a b =
  let va = eval prog env a in
  match op with
  | A.And -> (
      match va with
      | VBool false -> VBool false
      | VBool true -> eval prog env b
      | _ -> raise (Stuck "&&"))
  | A.Or -> (
      match va with
      | VBool true -> VBool true
      | VBool false -> eval prog env b
      | _ -> raise (Stuck "||"))
  | _ -> eval_binop2 op va (eval prog env b)

and eval_binop2 op va vb =
  let num f g =
    match (va, vb) with
    | VInt (x, bits, signed), VInt (y, _, _) ->
        VInt (f ~bits ~signed x y, bits, signed)
    | VFloat x, VFloat y -> VFloat (g x y)
    | _ -> raise (Stuck "arithmetic")
  in
  let cmp f =
    match (va, vb) with
    | VInt (x, _, signed), VInt (y, _, _) ->
        VBool
          (f (if signed then Int64.compare x y else Int64.unsigned_compare x y))
    | VFloat x, VFloat y -> VBool (f (Float.compare x y))
    | _ -> raise (Stuck "comparison")
  in
  match op with
  | A.Add -> num Num.add ( +. )
  | A.Sub -> num Num.sub ( -. )
  | A.Mul -> num Num.mul ( *. )
  | A.Div -> num Num.div ( /. )
  | A.Mod -> num Num.rem Float.rem
  | A.Lt -> cmp (fun c -> c < 0)
  | A.Gt -> cmp (fun c -> c > 0)
  | A.Le -> cmp (fun c -> c <= 0)
  | A.Ge -> cmp (fun c -> c >= 0)
  | A.Eq -> VBool (va = vb)
  | A.Ne -> VBool (va <> vb)
  | A.Concat -> (
      match (va, vb) with
      | VStr x, VStr y -> VStr (x ^ y)
      | _ -> raise (Stuck "++"))
  | A.And | A.Or -> raise (Stuck "logical")

and eval_call prog env f args =
  match builtin prog env f args with
  | Some v -> v
  | None -> apply prog f (List.map (eval prog env) args)

(* Apply a named function to already-evaluated argument values. *)
and apply prog name (vals : value list) : value =
  match
    List.find_opt (fun (fd : A.fn_def) -> String.equal fd.name name) prog
  with
  | Some fd ->
      let env = List.combine (List.map fst fd.params) vals in
      eval prog env fd.body
  | None -> raise (Stuck ("unknown function " ^ name))

and builtin prog env name args : value option =
  (* [find]'s second argument is a function name, not a value, so it is handled
     before the other arguments are evaluated. *)
  match (name, args) with
  | "find", [ l; { A.kind = A.Var fn; _ } ] -> (
      match eval prog env l with
      | VList xs ->
          Some
            (VOpt
               (List.find_opt
                  (fun x ->
                    match apply prog fn [ x ] with VBool b -> b | _ -> false)
                  xs))
      | _ -> raise (Stuck "find"))
  | _ -> builtin_v name (List.map (eval prog env) args)

and builtin_v name (vals : value list) : value option =
  match (name, vals) with
  | "length", [ VList l ] -> Some (vint (List.length l))
  | "head", [ VList l ] ->
      Some (VOpt (match l with x :: _ -> Some x | [] -> None))
  | "get", [ VList l; VInt (i, _, _) ] ->
      Some (VOpt (List.nth_opt l (Int64.to_int i)))
  | "lookup", [ VMap m; k ] ->
      Some
        (VOpt
           (List.find_map (fun (kk, vv) -> if kk = k then Some vv else None) m))
  | "get_or", [ VMap m; k; d ] ->
      Some
        (match
           List.find_map (fun (kk, vv) -> if kk = k then Some vv else None) m
         with
        | Some vv -> vv
        | None -> d)
  | "to_int", [ VFloat f ] ->
      Some
        (VOpt
           (match Num.to_int f with
           | Some n -> Some (VInt (n, 64, true))
           | None -> None))
  | "to_float", [ VInt (n, _, signed) ] ->
      Some (VFloat (Num.to_float ~signed n))
  | ( ( "length" | "head" | "get" | "find" | "lookup" | "get_or" | "to_int"
      | "to_float" ),
      _ ) ->
      raise (Stuck ("builtin " ^ name))
  | _ -> None

and eval_match prog env scrut arms =
  let tag, payload =
    match scrut with
    | VVariant (t, p) -> (t, p)
    | _ -> raise (Stuck "match subject")
  in
  let rec pick = function
    | (A.PVariant { variant; bind }, body) :: rest ->
        if String.equal variant tag then
          let env =
            match bind with Some b -> (b, payload) :: env | None -> env
          in
          eval prog env body
        else pick rest
    | (A.PUnknown { bind }, body) :: _ ->
        eval prog ((bind, VStr tag) :: env) body
    | [] -> raise (Stuck "non-exhaustive match")
  in
  pick arms

(* Evaluate a named entry function with the given argument values. *)
let eval_fn prog name vals = apply prog name vals
