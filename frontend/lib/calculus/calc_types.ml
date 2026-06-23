(* The calculus type system. It mirrors the IR's type algebra (primitives,
   nominal references, lists, maps) and adds three things the IR has no notion
   of: [Opt] (the [T?] Option that partial builtins and [??] work over -- in the
   IR nullability is a member flag, not a type), [Fn] (the internal type of a
   named function passed to a combinator, which never crosses to the IR/wire),
   and [Int_lit] (an integer literal whose width is inferred from context). [Err]
   is an already-diagnosed type that is compatible with everything, so a reported
   error does not cascade into a flood of follow-on diagnostics. *)

type t =
  | Prim of Ir.prim
  | Ref of string * t list
  | List of t
  | Map of t * t
  | Opt of t
  | Fn of t list * t
  | Int_lit
  | Err

(* Surface boundary type to calculus type. Reuses the lowering primitive map and
   turns a [T?] annotation into [Opt]. *)
let rec resolve (ty : Ast.ty) : t =
  match ty with
  | Ast.TPrim (kw, _) -> Prim (Lower.Internal.prim_of_keyword kw)
  | Ast.TName (name, args, _) -> Ref (name, List.map resolve args)
  | Ast.TList (elem, _) -> List (resolve elem)
  | Ast.TMap (k, v, _) -> Map (resolve k, resolve v)
  | Ast.TNullable (inner, _) -> Opt (resolve inner)
  | Ast.TError _ -> Err

let prim_name : Ir.prim -> string = function
  | Ir.Bool -> "bool"
  | Ir.String -> "string"
  | Ir.Bytes -> "bytes"
  | Ir.Int { bits; signed } ->
      Printf.sprintf "%c%d" (if signed then 'i' else 'u') bits
  | Ir.Float -> "float"
  | Ir.Timestamp -> "timestamp"
  | Ir.Date -> "date"
  | Ir.Duration -> "duration"
  | Ir.Uuid -> "uuid"

let rec to_string : t -> string = function
  | Prim p -> prim_name p
  | Ref (n, []) -> n
  | Ref (n, args) ->
      Printf.sprintf "%s[%s]" n (String.concat ", " (List.map to_string args))
  | List t -> "[]" ^ to_string t
  | Map (k, v) -> Printf.sprintf "map[%s]%s" (to_string k) (to_string v)
  | Opt t -> to_string t ^ "?"
  | Fn (ps, r) ->
      Printf.sprintf "(%s) -> %s"
        (String.concat ", " (List.map to_string ps))
        (to_string r)
  | Int_lit -> "int"
  | Err -> "<error>"

let i64 = Prim (Ir.Int { bits = 64; signed = true })
let is_int = function Prim (Ir.Int _) -> true | Int_lit -> true | _ -> false
let is_numeric = function Prim Ir.Float -> true | t -> is_int t

(* Structural compatibility: is a value of type [a] acceptable where [b] is
   expected? [Err] matches anything (its error was already reported); an integer
   literal matches any integer width. *)
let rec compat (a : t) (b : t) : bool =
  match (a, b) with
  | Err, _ | _, Err -> true
  | Int_lit, Prim (Ir.Int _) | Prim (Ir.Int _), Int_lit | Int_lit, Int_lit ->
      true
  | Prim p, Prim q -> p = q
  | Ref (n1, a1), Ref (n2, a2) ->
      String.equal n1 n2
      && List.length a1 = List.length a2
      && List.for_all2 compat a1 a2
  | List x, List y -> compat x y
  | Map (k1, v1), Map (k2, v2) -> compat k1 k2 && compat v1 v2
  | Opt x, Opt y -> compat x y
  | Fn (p1, r1), Fn (p2, r2) ->
      List.length p1 = List.length p2
      && List.for_all2 compat p1 p2 && compat r1 r2
  | _ -> false
