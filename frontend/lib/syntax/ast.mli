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
  | AFloat of float
  | AName of string
  | AKv of string * trait_arg

type trait = { tname : string; targs : trait_arg list; tspan : Span.span }

type member = {
  mname : string;
  mname_span : Span.span;
  mtype : ty;
  mtraits : trait list;
}

type enum_case = {
  cname : string;
  cname_span : Span.span;
  cint : int option;
  ctraits : trait list;
}

type union_variant = {
  vname : string;
  vname_span : Span.span;
  vpayload : ty option;
  vtraits : trait list;
}

type decl_kind =
  | DStruct of { params : string list; members : member list }
  | DEnum of { cases : enum_case list }
  | DUnion of { params : string list; variants : union_variant list }
  | DOp of { input : ty option; output : ty option }

type decl = {
  dname : string;
  dname_span : Span.span;
  pub : bool;
  dtraits : trait list;
  dkind : decl_kind;
}

type file = decl list

val ty_span : ty -> Span.span
