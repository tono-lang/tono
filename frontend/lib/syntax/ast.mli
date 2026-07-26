(* Surface AST produced by the parser and consumed by [Lower]. *)

type ty =
  | TPrim of string * Span.span
  | TName of string * ty list * Span.span
  | TQName of string * string * ty list * Span.span
  | TList of ty * Span.span
  | TMap of ty * ty * Span.span
  | TNullable of ty * Span.span
  | TError of Span.span

type ref_path = { segs : string list; ref_span : Span.span }

type trait_arg =
  | AString of string
  | AInt of int
  | AFloat of float
  | AName of string
  | ARef of ref_path
  | AKv of string * trait_arg

type trait = { tname : string; targs : trait_arg list; tspan : Span.span }

type match_pattern =
  | PString of string
  | PInt of int
  | PName of string
  | PWildcard

type arm_value =
  | AVRef of ref_path
  | AVString of string
  | AVInt of int
  | AVName of string
  | AVSources of trait list

type match_arm = {
  pat : match_pattern;
  pat_span : Span.span;
  value : arm_value;
  value_span : Span.span;
}

type field_match = {
  subject : ref_path;
  arms : match_arm list;
  match_span : Span.span;
}

type member = {
  mname : string;
  mname_span : Span.span;
  mtype : ty;
  mmatch : field_match option;
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

type ext_kind = EHook | EContract | EConstraint
type ext_binding = { lang : string; lang_span : Span.span; target : string }
type ext_sig = { esig_in : ty; esig_out : ty }

type decl_kind =
  | DStruct of { params : string list; members : member list; ops : decl list }
  | DEnum of { cases : enum_case list }
  | DUnion of { params : string list; variants : union_variant list }
  | DOp of { input : ty option; output : ty option }
  | DExt of {
      ekind : ext_kind;
      ekind_span : Span.span;
      esig : ext_sig option;
      ebindings : ext_binding list;
      econformance : string option;
    }

and decl = {
  dname : string;
  dname_span : Span.span;
  pub : bool;
  dtraits : trait list;
  dkind : decl_kind;
}

type import = {
  imported_path : string list;
  alias : string option;
  ispan : Span.span;
}

type file = { imports : import list; decls : decl list }

val ty_span : ty -> Span.span
