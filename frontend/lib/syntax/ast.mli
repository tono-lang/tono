(* Surface AST produced by the parser and consumed by [Lower]. *)

type ty =
  | TPrim of string * Span.span
  | TName of string * ty list * Span.span
  | TList of ty * Span.span
  | TMap of ty * ty * Span.span
  | TNullable of ty * Span.span
  | TError of Span.span

type trait_arg =
  | AString of string
  | AInt of int
  | AName of string
  | AKv of string * trait_arg

type trait = { tname : string; targs : trait_arg list; tspan : Span.span }

type member = {
  mname : string;
  mname_span : Span.span;
  mtype : ty;
  mtraits : trait list;
}

type decl_kind = DStruct of { params : string list; members : member list }

type decl = {
  dname : string;
  dname_span : Span.span;
  pub : bool;
  dtraits : trait list;
  dkind : decl_kind;
}

type file = decl list

val ty_span : ty -> Span.span
