(* Surface AST. The parser builds this directly from tokens; [Lower] maps it to
   the PRD-defined IR. Keeping the AST separate from the IR keeps the parser
   purely about syntax and isolates the surface-to-IR contract in one place. *)

type ty =
  | TPrim of string * Span.span (* a primitive keyword as written, e.g. "i64" *)
  | TName of string * ty list * Span.span (* Name, or Name[args] application *)
  | TQName of string * string * ty list * Span.span
    (* qualifier.Name, or qualifier.Name[args] — a reference to another module's
       shape through an import qualifier (its alias or last path segment) *)
  | TList of ty * Span.span (* []T *)
  | TMap of ty * ty * Span.span (* map[K]V *)
  | TNullable of ty * Span.span (* T? *)
  | TError of Span.span (* a type position that failed to parse *)

(* A field reference written [.a] or a path [.a.b]: the segments after the
   leading dot, in order. Refs are resolved against the enclosing entry (or
   config) by the typechecker; the parser only records the path. *)
type ref_path = { segs : string list; ref_span : Span.span }

type trait_arg =
  | AString of string
  | AInt of int
  | AFloat of float
  | AName of string (* an identifier argument: a type/name ref or HTTP method *)
  | ARef of ref_path (* a field reference argument, e.g. @env(.endpoint_env) *)
  | AKv of string * trait_arg (* key: value, e.g. @range(min: 0) *)

type trait = { tname : string; targs : trait_arg list; tspan : Span.span }

(* One pattern of a field-selection match: a literal of the subject's type, a
   bare name (bool literal or enum case), or the [_] wildcard. *)
type match_pattern =
  | PString of string
  | PInt of int
  | PName of string
  | PWildcard

(* What a selected arm yields: another field, a literal, or a stack of value
   sources ([@env]/[@default]) resolved in place. *)
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

(* [field: T = match .subject { pat => value ... }] — the only selection form. *)
type field_match = {
  subject : ref_path;
  arms : match_arm list;
  match_span : Span.span;
}

type member = {
  mname : string;
  mname_span : Span.span;
  mtype : ty;
  mmatch : field_match option; (* [= match ...] selection, entry/config only *)
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

(* One variant of a union: a name, an optional payload type, and trailing traits.
   Lowers to an IR member (name = variant, target = payload). *)
type union_variant = {
  vname : string;
  vname_span : Span.span;
  vpayload : ty option;
  vtraits : trait list;
}

(* The bespoke extension flavours. [EHook] fills a fixed lifecycle slot;
   [EContract]/[EConstraint] are named with a typed signature; [EImpl] names the
   operation it implements and takes its signature from that operation. *)
type ext_kind = EHook | EContract | EConstraint | EImpl

(* One "lang: file#symbol" entry in an extension body. *)
type ext_binding = { lang : string; lang_span : Span.span; target : string }

(* A contract/constraint signature: (input) -> output. Hooks omit it. *)
type ext_sig = { esig_in : ty; esig_out : ty }

type decl_kind =
  | DStruct of { params : string list; members : member list; ops : decl list }
    (* [ops] are operations declared in the struct body (an "entry"); each is a
       full decl with [dkind = DOp]. A plain data struct has none. *)
  | DEnum of { cases : enum_case list }
  | DUnion of { params : string list; variants : union_variant list }
  | DOp of { input : ty option; output : ty option }
  | DExt of {
      ekind : ext_kind;
      ekind_span : Span.span;
      esig : ext_sig option;
      eraw : Span.span option;
          (* the span of the "raw" word when written, so the checker can point at
             it; [None] means the typed form *)
      ebindings : ext_binding list;
      econformance : string option;
    }

and decl = {
  dname : string;
  dname_span : Span.span;
  pub : bool;
  dtraits : trait list; (* shape-level traits written before the keyword *)
  dkind : decl_kind;
}

(* An import brings another module into scope under a qualifier. The qualified
   path names the target module (["payments"; "common"] for [import
   payments.common]); the qualifier used in references is the alias, when given,
   otherwise the last path segment. *)
type import = {
  imported_path : string list;
  alias : string option;
  ispan : Span.span;
}

type file = { imports : import list; decls : decl list }

let ty_span : ty -> Span.span = function
  | TPrim (_, s)
  | TName (_, _, s)
  | TQName (_, _, _, s)
  | TList (_, s)
  | TMap (_, _, s)
  | TNullable (_, s)
  | TError s ->
      s
