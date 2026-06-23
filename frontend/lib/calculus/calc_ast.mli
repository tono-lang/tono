(* The calculus surface AST: the fixed, total expression language behind the
   @computed / @validate traits. Boundary types reuse the surface grammar
   [Ast.ty]; every expression node carries a span for diagnostics. *)

type lit =
  | Int of { value : string; width : int; signed : bool }
  | Float of float
  | Str of string
  | Bool of bool

type binop =
  | Add
  | Sub
  | Mul
  | Div
  | Mod
  | Eq
  | Ne
  | Lt
  | Gt
  | Le
  | Ge
  | And
  | Or
  | Concat

type pattern =
  | PVariant of { variant : string; bind : string option }
  | PUnknown of { bind : string }

type expr = { kind : expr_kind; span : Span.span }

and expr_kind =
  | Lit of lit
  | Var of string
  | Field of expr * string
  | Not of expr
  | Binop of binop * expr * expr
  | If of expr * expr * expr
  | Let of string * expr * expr
  | Call of string * expr list
  | Map of expr * string
  | Filter of expr * string
  | Fold of expr * expr * string
  | Match of expr * (pattern * expr) list
  | Some_ of expr
  | None_
  | Coalesce of expr * expr
  | Ctor of string * (string * expr) list

type fn_def = {
  name : string;
  name_span : Span.span;
  params : (string * Ast.ty) list;
  ret : Ast.ty;
  body : expr;
}

type program = fn_def list
