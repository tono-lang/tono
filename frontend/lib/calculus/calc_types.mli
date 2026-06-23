(* The calculus type system: the IR type algebra plus [Opt] (the [T?] Option),
   [Fn] (internal function type for combinator arguments), [Int_lit] (a
   width-polymorphic integer literal), and [Err] (an already-diagnosed type
   compatible with everything, to stop error cascades). *)

type t =
  | Prim of Ir.prim
  | Ref of string * t list
  | List of t
  | Map of t * t
  | Opt of t
  | Fn of t list * t
  | Int_lit
  | Err

(* Resolve a surface boundary type ([Ast.ty]) to a calculus type. *)
val resolve : Ast.ty -> t

(* Render a type for diagnostics. *)
val to_string : t -> string

(* The default integer type ([i64]). *)
val i64 : t

(* Whether a type is an integer (a sized int or an integer literal). *)
val is_int : t -> bool

(* Whether a type is numeric (an integer or float). *)
val is_numeric : t -> bool

(* Structural compatibility: a value of the first type is acceptable where the
   second is expected. [Err] matches anything; an integer literal matches any
   integer width. *)
val compat : t -> t -> bool
