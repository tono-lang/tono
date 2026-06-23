(* The calculus surface AST: the fixed, total expression language behind the
   @computed / @validate traits. Every expression node carries a span so the
   checker can point diagnostics at source. Boundary types -- a function's
   parameter and return types -- reuse the surface type grammar [Ast.ty]; the
   checker resolves those against the IR and the calculus type system. *)

type lit =
  (* The integer value is kept as text to preserve i64 without host precision
     loss in the frontend (mirrors the wire format). *)
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
  | Concat (* ++ , string concatenation only *)

type pattern =
  | PVariant of { variant : string; bind : string option }
  | PUnknown of { bind : string }
(* the mandatory arm for an open sum *)

type expr = { kind : expr_kind; span : Span.span }

and expr_kind =
  | Lit of lit
  | Var of string
  | Field of expr * string
  | Not of expr
  | Binop of binop * expr * expr
  | If of expr * expr * expr
  | Let of string * expr * expr
  | Call of string * expr list (* a named fn or a builtin *)
  | Map of expr * string (* list, named fn *)
  | Filter of expr * string (* list, named fn *)
  | Fold of expr * expr * string (* list, init, named fn *)
  | Match of expr * (pattern * expr) list
  | Some_ of expr
  | None_
  | Coalesce of expr * expr (* ?? *)
  | Ctor of string * (string * expr) list (* struct value or union variant *)

(* A function: named parameters with surface types, a surface return type, and a
   body expression. [name_span] anchors definition-level diagnostics. *)
type fn_def = {
  name : string;
  name_span : Span.span;
  params : (string * Ast.ty) list;
  ret : Ast.ty;
  body : expr;
}

type program = fn_def list
