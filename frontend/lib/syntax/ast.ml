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
type ref_path = {
  segs : string list;
  index : ref_path option;
  ref_span : Span.span;
}

type trait_arg =
  | AString of string
  | AInt of int
  | AFloat of float
  | AName of string (* an identifier argument: a type/name ref or HTTP method *)
  | ARef of ref_path (* a field reference argument, e.g. @env(.endpoint_env) *)
  | AKv of string * trait_arg (* key: value, e.g. @range(min: 0) *)
  | AList of
      trait_arg list (* a list literal value, e.g. @http(code: [200, 207]) *)
  | ACtor of ctor_arg
    (* a struct-literal mapper, e.g. @body(note_body { title: .input.title }) *)
  | ACall of call_expr
(* an extern call, e.g. @header("Authorization", companyauth.sign(.request)) *)

and ctor_arg = {
  ctor_name : string;
  ctor_name_span : Span.span;
  ctor_fields : (string * Span.span * trait_arg) list;
  ctor_span : Span.span;
}

(* A bare scalar literal argument, e.g. the "notes" in [.bus.send("notes",
   .payload.body)]: [call_arg]'s own literal case, distinct from
   [trait_arg]'s (a different grammar position with a different set of
   argument forms). *)
and call_lit = LStr of string | LInt of int | LFloat of float

(* One argument to a call expression (see [call_expr]): the caller's own
   parameter by name, a field-reference path, a struct-literal mapper — the
   same shape [ctor_arg] already gives [@body(name { field: value })] — or a
   bare literal. *)
and call_arg =
  | CaParam of string * Span.span
  | CaRef of ref_path
  | CaCtor of ctor_arg
  | CaLit of call_lit * Span.span

(* "ns.fn(args)": a call into an [extern] declared in the [ext] block named
   [ce_ns]. Shared by three surface positions: a member's [= ns.fn(...)]
   value, a trait argument (e.g. inside [@header]), and a language block's
   [call:] line reuses only the argument-list grammar, not this whole node. *)
and call_expr = {
  ce_ns : string;
  ce_fn : string;
  ce_head_span : Span.span;
  ce_args : call_arg list;
  ce_span : Span.span;
}

type trait = { tname : string; targs : trait_arg list; tspan : Span.span }

(* One pattern of a field-selection match: a literal of the subject's type, a
   bare name (bool literal or enum case), or the [_] wildcard. *)
type match_pattern =
  | PString of string
  | PInt of int
  | PName of string
  | PWildcard
  | PNull

(* What a selected arm yields: another field, a literal, or a stack of value
   sources ([@env]/[@default]) resolved in place. *)
type arm_value =
  | AVRef of ref_path
  | AVString of string
  | AVInt of int
  | AVName of string
  | AVSources of trait list
  | AVSubject of Span.span

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

(* A call into a declared opaque handle's method, [.field.method(args)]:
   the receiver names an entry field (a declared opaque handle), the method
   one of its declared [extern] methods. Two surface positions share the
   node: an op's own [impl .h.m(args)] body (a third implementation source
   for an op, alongside a protocol trait (@http) and a legacy "ext impl"
   binding) and a member's [= .h.m(args)] value source (a foreign resolution
   that feeds several operations, the receiver-form counterpart of
   [= ns.fn(args)]). The closed-boundary/arity rules against the handle's
   declared method live in the typechecker, not here. *)
type op_impl = {
  oi_recv : ref_path;
  oi_method : string;
  oi_method_span : Span.span;
  oi_args : call_arg list;
  oi_span : Span.span;
}

(* A member's [= ...] value: the existing selection table, an extern call
   (e.g. [config: app_config = companyconfig.load(.service, .region)]), or a
   handle method call (e.g. [config: app_config = .provider.get()]). *)
type member_value =
  | MMatch of field_match
  | MCall of call_expr
  | MHandleCall of op_impl

type member = {
  mname : string;
  mname_span : Span.span;
  mtype : ty;
  mvalue : member_value option; (* [= ...], entry/config only *)
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

(* ── FFI library blocks: ext <name> { ... } ──────────────────────────────
   The new form of [ext], distinct from the legacy hook/contract/constraint/
   impl kinds above. Declares where a third-party library lives per
   language, its foreign shapes, and the extern functions/methods that call
   into it. Surface-and-IR only for now: no typecheck resolves these against
   each other yet (that is a later task; see [Lower.lower_ext_lib]). *)

(* "lang: "module/path"" — one per-language module path. *)
type lang_path = {
  lp_lang : string;
  lp_lang_span : Span.span;
  lp_path : string;
}

(* A field of a foreign struct, named and cased exactly as the foreign side
   spells it (PascalCase for Go, etc. — never normalized). *)
type foreign_field = {
  ff_name : string;
  ff_name_span : Span.span;
  ff_type : ty;
}

(* [struct go_config { Host: string, ... }] inside an [ext] block: a foreign
   shape, never a top-level [DStruct] and never role-classified. *)
type foreign_struct = {
  fs_name : string;
  fs_name_span : Span.span;
  fs_fields : foreign_field list;
  fs_span : Span.span;
}

(* A [yields:] position's type: an ordinary tono type, or the reserved
   [error] sentinel — valid only here, nowhere else in the grammar. *)
type yields_ty = YType of ty | YError of Span.span

type yields_pos = {
  yp_name : string;
  yp_name_span : Span.span;
  yp_ty : yields_ty;
}

(* One field of a [returns: Type { field: value, ... }] literal: a bare ref
   into a [yields:]-bound name, or a match over one. *)
type returns_value = RvRef of ref_path | RvMatch of field_match

type returns_field = {
  rf_name : string;
  rf_name_span : Span.span;
  rf_value : returns_value;
  rf_span : Span.span;
}

type returns_lit = {
  rl_type : ty;
  rl_fields : returns_field list;
  rl_span : Span.span;
}

(* One [errors: { "sentinel" => TypeName }] entry. *)
type error_map_entry = {
  em_sentinel : string;
  em_sentinel_span : Span.span;
  em_type : string;
  em_type_span : Span.span;
}

(* One per-language block inside an [extern]'s body. [call:] is mandatory
   (its absence is diagnosed, not encoded as an option); the rest are
   optional lines. *)
type extern_lang_body = {
  elb_lang : string;
  elb_lang_span : Span.span;
  elb_call_symbol : string;
  elb_call_symbol_span : Span.span;
  elb_call_args : call_arg list;
  elb_yields : yields_pos list option;
  elb_returns : returns_lit option;
  elb_errors : error_map_entry list;
  elb_sync : bool;
  (* Marks a call with no error return (Go's own convention: every call is
     fallible by default, `(value, error)`; this opts a single-return
     foreign function like [uuid.NewString() string] out of that shape).
     Other targets ignore it the same way Go ignores [sync]. *)
  elb_infallible : bool;
  (* Opts the call into receiving the target's own cancellation/deadline
     context in its idiomatic position (Go: `ctx context.Context` as the
     first parameter). Only meaningful on a foreign handle's own method
     call; targets that have not landed the equivalent convention ignore
     it the same way Go ignores [sync]. *)
  elb_ctx : bool;
  elb_span : Span.span;
}

type extern_param = { ep_name : string; ep_name_span : Span.span; ep_type : ty }

(* [extern name(params): type { lang { ... } ... }] — a free function inside
   an [ext] block, or a method inside a [type] opaque-handle block. *)
type extern_decl = {
  ed_name : string;
  ed_name_span : Span.span;
  ed_params : extern_param list;
  ed_return : ty;
  ed_langs : extern_lang_body list;
  ed_span : Span.span;
}

(* [type name("ForeignName", Arg) { ... }] — declares which instantiation of
   a foreign generic type this opaque handle names: the foreign type's own
   name (a string, so the origin stays visible) and the tono type argument
   it is monomorphized with. Absent for a foreign type that is not generic;
   the handle then keeps being written as [type name { ... }] and its
   foreign identifier is derived from [opq_name] itself, as before. *)
type opaque_instance = {
  oi_foreign_name : string;
  oi_foreign_span : Span.span;
  oi_arg : ty;
  oi_arg_span : Span.span;
  oi_span : Span.span;
}

(* [type name { extern ... }] — an opaque foreign handle whose only members
   are extern methods; never serializes, never crosses the wire. The
   [interface] marker declares the foreign type is abstract (a Go interface,
   held by value), not a concrete struct held by pointer; tono cannot infer
   that from the foreign name alone, and the two spell differently in Go. *)
type opaque_type = {
  opq_name : string;
  opq_name_span : Span.span;
  opq_instance : opaque_instance option;
  opq_interface : bool;
  opq_methods : extern_decl list;
  opq_span : Span.span;
}

(* The body of [ext name { ... }]. *)
type ext_lib_body = {
  elib_langs : lang_path list;
  elib_structs : foreign_struct list;
  elib_types : opaque_type list;
  elib_externs : extern_decl list; (* free (non-method) externs *)
}

(* ── Declared tests ────────────────────────────────────────────────────── *)

(* The head of a test-block constructor: a shape name ([user]) or a qualified
   language-module shape ([http.response], [errors.api]) as written, so the
   checker resolves it against the module or the imported tono.* surface. *)
type value_head = { vh_segs : string list; vh_span : Span.span }

(* A value in a test block: the closed literal/ctor/list subset of the calculus
   value forms, plus a binding reference ([saved.id]) for dataflow between
   bindings and a braced map literal (needed for header maps). There is no
   [Some]: a plain value fills an optional member directly, and an omitted
   optional member is absent. *)
type test_value =
  | TvStr of string * Span.span
  | TvInt of int * Span.span
  | TvFloat of float * Span.span
  | TvBool of bool * Span.span
  | TvCtor of test_ctor
  | TvList of test_value list * Span.span
  | TvMap of ((string * Span.span) * test_value) list * Span.span
  | TvRef of { base : string; path : string list; ref_span : Span.span }
  | TvError of Span.span

and test_ctor = {
  tc_head : value_head;
  tc_fields : (string * Span.span * test_value) list;
  tc_span : Span.span;
}

(* A pattern in an [expect]: ctor-like with the three extra marks ([..] frees
   the uncited fields, [any] asserts presence, [None] asserts absence), a bare
   [ok], a literal (equality), a list (the [.requests] form), or a braced map
   subset (header maps). Patterns are test grammar, not calculus expressions. *)
type test_pattern =
  | TpCtor of test_pattern_ctor
  | TpLit of test_value
  | TpOk of Span.span
  | TpList of test_pattern list * Span.span
  | TpMap of {
      entries : ((string * Span.span) * test_pattern_field) list;
      map_open : bool;
      map_span : Span.span;
    }
  | TpError of Span.span

and test_pattern_ctor = {
  tp_head : value_head;
  tp_fields : (string * Span.span * test_pattern_field) list;
  tp_open : bool;
  tp_span : Span.span;
}

and test_pattern_field =
  | TpfPat of test_pattern
  | TpfAny of Span.span
  | TpfAbsent of Span.span

(* [stub c.get_user.http]: a dotted path of 2 or 3 identifiers. Typecheck
   disambiguates the shape: a 3-segment path whose first segment names a
   construction binding is the client/op/dep form; otherwise the path names
   an "ext" library free function (2 segments) or opaque-handle method (3
   segments, qualified by the type name). *)
type stub_target = { st_path : (string * Span.span) list; st_span : Span.span }

type test_item =
  | TiConstruct of {
      bind : string;
      bind_span : Span.span;
      entry : string;
      entry_span : Span.span;
      fields : (string * Span.span * test_value) list;
      item_span : Span.span;
    }
  | TiStub of {
      bind : (string * Span.span) option;
      target : stub_target;
      value : test_value;
      item_span : Span.span;
    }
  | TiCall of {
      bind : string;
      bind_span : Span.span;
      recv : string;
      recv_span : Span.span;
      op : string;
      op_span : Span.span;
      input : test_value option;
      item_span : Span.span;
    }
  | TiExpect of {
      subject : string;
      subject_span : Span.span;
      requests : bool;
      pattern : test_pattern;
      item_span : Span.span;
    }

type decl_kind =
  | DStruct of { params : string list; members : member list; ops : decl list }
    (* [ops] are operations declared in the struct body (an "entry"); each is a
       full decl with [dkind = DOp]. A plain data struct has none. *)
  | DEnum of { cases : enum_case list }
  | DUnion of { params : string list; variants : union_variant list }
  | DOp of {
      pname : string option;
      input : ty option;
      output : ty option;
      oimpl : op_impl option;
    }
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
  | DExtLib of { body : ext_lib_body; span : Span.span }
  | DTest of { titems : test_item list }
(* a declared test block; [dname] is its free-form string name *)

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
