(* Surface AST. The parser builds this directly from tokens; [Lower] maps it to
   the PRD-defined IR. Keeping the AST separate from the IR keeps the parser
   purely about syntax and isolates the surface-to-IR contract in one place. *)

type ty =
  | TPrim of string * Span.span (* a primitive keyword as written, e.g. "i64" *)
  | TName of string * ty list * Span.span (* Name, or Name[args] application *)
  | TList of ty * Span.span (* []T *)
  | TMap of ty * ty * Span.span (* map[K]V *)
  | TNullable of ty * Span.span (* T? *)
  | TError of Span.span (* a type position that failed to parse *)

type trait_arg =
  | AString of string
  | AInt of int
  | AName of string (* an identifier argument: a type/name ref or HTTP method *)
  | AKv of string * trait_arg (* key: value, e.g. @range(min: 0) *)

type trait = { tname : string; targs : trait_arg list; tspan : Span.span }

type member = {
  mname : string;
  mname_span : Span.span;
  mtype : ty;
  mtraits : trait list;
}

(* One variant of an enum: a name, an optional [= N] for int-backed enums, and
   any trailing traits. *)
type enum_case = {
  cname : string;
  cname_span : Span.span;
  cint : int option;
  ctraits : trait list;
}

type decl_kind =
  | DStruct of { params : string list; members : member list }
  | DEnum of { cases : enum_case list }
  | DUnion of { params : string list; members : member list }
  | DOp of { input : ty option; output : ty option; errors : ty list }

type decl = {
  dname : string;
  dname_span : Span.span;
  pub : bool;
  dtraits : trait list; (* shape-level traits written before the keyword *)
  dkind : decl_kind;
}

type file = decl list

let ty_span : ty -> Span.span = function
  | TPrim (_, s)
  | TName (_, _, s)
  | TList (_, s)
  | TMap (_, _, s)
  | TNullable (_, s)
  | TError s ->
      s
