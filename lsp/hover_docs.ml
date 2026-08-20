(* What the editor *says*: the trait contracts, construct prose, and primitive
   notes behind hover, completion and signature help. Kept apart from
   [Analysis], which decides *where* the cursor is; this module holds no
   position logic and never touches a span.

   Every enumerated vocabulary here is rendered from the checker or grammar
   that enforces it (the @str:: catalog, the value sources, the ext block's
   words), so the editor can never explain a form the compiler does not
   accept. *)

module Ast = Tono_frontend.Ast
module Check_ext = Tono_frontend.Check_ext
module Check_entries = Tono_frontend.Check_entries
module Entry_scope = Tono_frontend.Entry_scope
module Ext_lib_vocab = Tono_frontend.Ext_lib_vocab
module Check_request_value = Tono_frontend.Check_request_value

(* The trait contracts surfaced on hover, offered after `@`, and expanded by
   signature help: one table, three consumers, so the documented keys can
   never drift between them. The frontend's central trait registry,
   [Trait_vocab.known], names the bare traits the compiler acts on but carries
   no prose, so this table cannot be derived from it outright; a test
   ([trait_docs_cover_the_compiler_vocabulary]) instead checks the two stay in
   step, in both directions. Keep entries in step with the checkers that read
   them (check_http, check_constraints, protocol_http). *)
type trait_info = { ti_doc : string; ti_keys : (string * string) list }

let trait_registry : (string * trait_info) list =
  [
    ( "doc",
      {
        ti_doc =
          "Documentation carried into every generated SDK as the target \
           language's doc comment.";
        ti_keys = [ ("text", "string") ];
      } );
    ( "http",
      {
        ti_doc = "Binds the operation to an HTTP endpoint.";
        ti_keys = [ ("method", "\"GET\" | \"POST\" | ..."); ("path", "string") ];
      } );
    ( "range",
      {
        ti_doc = "Numeric bounds validated at the boundary (inclusive).";
        ti_keys = [ ("min", "int"); ("max", "int") ];
      } );
    ( "length",
      {
        ti_doc = "String length bounds validated at the boundary (inclusive).";
        ti_keys = [ ("min", "int"); ("max", "int") ];
      } );
    ( "rename",
      {
        ti_doc =
          "Overrides the idiomatic identifier for one language, e.g. \
           @rename(go: \"ID\"). The wire key is unchanged.";
        ti_keys = [ ("lang", "string") ];
      } );
    ( "deprecated",
      {
        ti_doc =
          "Marks the element deprecated in every generated SDK, with the given \
           note.";
        ti_keys = [ ("note", "string") ];
      } );
    ( "status",
      {
        ti_doc = "The HTTP status this error shape is discriminated by.";
        ti_keys = [ ("code", "int") ];
      } );
    ( "errorCode",
      {
        ti_doc =
          "The dotted path to probe in the response body, and the value it \
           must equal, to select this error shape when its status is shared.";
        ti_keys = [ ("path", "string"); ("value", "string") ];
      } );
    ( "errors",
      {
        ti_doc = "Declares the error shapes an operation can raise.";
        ti_keys = [ ("shapes", "name, ...") ];
      } );
    ( "async",
      {
        ti_doc =
          "Generates the asynchronous variant of the operation in targets that \
           distinguish it.";
        ti_keys = [];
      } );
    ( "discriminator",
      {
        ti_doc =
          "The union's tag field on the wire, e.g. @discriminator(\"kind\").";
        ti_keys = [ ("field", "string") ];
      } );
    ("retryable", { ti_doc = "Marks the error as safe to retry."; ti_keys = [] });
    ( "required",
      {
        ti_doc =
          "The member must be present. Absent by default, which is the \
           two-state nullability the wire carries.";
        ti_keys = [];
      } );
    ( "pattern",
      {
        ti_doc =
          "Regular expression a string must match, validated at the boundary.";
        ti_keys = [ ("", "string") ];
      } );
    ( "multipleOf",
      {
        ti_doc = "The number must be an exact multiple of this divisor.";
        ti_keys = [ ("", "int") ];
      } );
    ( "wire",
      {
        ti_doc =
          "The member's serialization key. Overrides the wire name only, never \
           the identifier the generated code exposes.";
        ti_keys = [ ("", "string") ];
      } );
    ( "httpResponseCode",
      {
        ti_doc = "The member receives the response's HTTP status code.";
        ti_keys = [];
      } );
    ( "entries",
      {
        ti_doc =
          "Serializes a map as an array of key/value pairs, escaping keys that \
           cannot be object keys.";
        ti_keys = [];
      } );
    (* The entry model: value sources, derivation, composition, and the
       per-operation protocol vocabulary. *)
    ( "arg",
      {
        ti_doc =
          "Required, passed explicitly: the field becomes a positional \
           constructor argument. Excludes every other source.";
        ti_keys = [];
      } );
    ( "with",
      {
        ti_doc =
          "Optional and configurable: the field becomes an option (Go WithX, \
           TS config field, Rust setter). Without a fallback, construction \
           fails when it is not supplied.";
        ti_keys = [];
      } );
    ( "env",
      {
        ti_doc =
          "Reads the value from an environment variable at construction. \
           Stackable: the declared order is the fallback chain. The name can \
           be a literal or a reference to a sibling field.";
        ti_keys = [ ("name", "string | .field") ];
      } );
    ( "default",
      {
        ti_doc = "The last resort of a source chain.";
        ti_keys = [ ("value", "literal") ];
      } );
    ( "format",
      {
        ti_doc =
          "Derives the field from a verbatim template. `{.field}` reads a \
           sibling field; nothing is normalized implicitly.";
        ti_keys = [ ("template", "string") ];
      } );
    ( "bind",
      {
        ti_doc =
          "Binds a field of a composed config to a value of this entry, at the \
           composition point.";
        ti_keys = [ ("target", "name"); ("source", ".field") ];
      } );
    ( "header",
      {
        ti_doc =
          "A request header. Key and value each accept a literal, a field \
           reference, or a template of references.";
        ti_keys = [ ("key", "string | .field"); ("value", "string | .field") ];
      } );
    ( "query",
      {
        ti_doc =
          "A query string parameter. Key and value each accept a literal, a \
           field reference, or a template of references.";
        ti_keys = [ ("key", "string | .field"); ("value", "string | .field") ];
      } );
    ( "body",
      {
        ti_doc =
          "The request body: a reference (the whole parameter, or one of its \
           members), or a struct-literal ctor mapping declared fields to \
           references. Absent means the operation sends no body.";
        ti_keys = [ ("", ".field | name { field: value, ... }") ];
      } );
    ( "timeout",
      {
        ti_doc = "Per-attempt timeout, read from the referenced field.";
        ti_keys = [ ("field", ".field") ];
      } );
    ( "retry",
      {
        ti_doc =
          "Maximum retries, read from the referenced field. Retryable = the \
           operation's @retryable errors plus transport failure; the backoff \
           is the runtime's.";
        ti_keys = [ ("field", ".field") ];
      } );
  ]

(* The @str::* catalog is closed and documented by the checker that enforces
   it; the registry names each entry from that one list. *)
let str_catalog_doc (name : string) : string =
  let what =
    match name with
    | "trim" -> "Strips leading and trailing whitespace."
    | "upper_snake" -> "Rewrites the value as UPPER_SNAKE_CASE."
    | "snake" -> "Rewrites the value as snake_case."
    | "kebab" -> "Rewrites the value as kebab-case."
    | "pascal" -> "Rewrites the value as PascalCase."
    | "lower" -> "Lowercases the value."
    | "upper" -> "Uppercases the value."
    | _ -> "A string transform."
  in
  what
  ^ " Part of the closed @str:: catalog, applied to the resolved value in the \
     order the transforms are declared."

let str_catalog : (string * trait_info) list =
  List.map
    (fun name ->
      ("str::" ^ name, { ti_doc = str_catalog_doc name; ti_keys = [] }))
    Check_entries.str_transforms

(* The hover prose for a trait: its contract plus the keys it takes, rendered
   from the same registry entry. *)
let trait_doc_text (info : trait_info) : string =
  match info.ti_keys with
  | [] -> info.ti_doc
  | keys ->
      info.ti_doc ^ " Keys: "
      ^ String.concat ", " (List.map (fun (k, v) -> k ^ ": " ^ v) keys)
      ^ "."

let trait_docs : (string * string) list =
  List.map
    (fun (name, i) -> (name, trait_doc_text i))
    (trait_registry @ str_catalog)

(* The "ext" library block: the contextual words the parser recognizes by
   position ([Ext_lib_vocab]), each with its contract. Consumed by hover and
   by completion inside a block, and checked against the vocabulary by
   [ext_lib_docs_cover_the_grammar]. The `.request` entry renders where the
   reference is legal from the checker that enforces it, so the two never
   disagree. *)
let ext_lib_docs : (string * string) list =
  [
    ( "extern",
      "A foreign function the SDK calls: a tono-facing signature (typed \
       parameters and a return type) plus one language block per target, each \
       declaring the real call and how its result becomes the declared return. \
       Free in an ext body, or a method inside a 'type' handle." );
    ( "type",
      "An opaque foreign handle: a value the library returns and the SDK only \
       passes back to it. Its members are extern methods; it never serializes \
       and never crosses the wire. type Name(\"Foreign\", Arg) { ... } names \
       which instantiation of a foreign generic type the handle declares: the \
       foreign type's own name as a string, plus the already-declared tono \
       type it is monomorphized with. When the targets spell that type \
       differently, name it per language in the module-path form: type \
       Name(go: \"GoName\", rust: \"RustName\", Arg) { ... }, one entry per \
       declared language. Omit the clause for a foreign type that is not \
       generic." );
    ( "interface",
      "Marks the foreign type as abstract: the handle is the library's \
       abstraction, not one concrete struct. In Go the constructors return the \
       interface value itself, so the generated code holds and passes it by \
       value; in Rust the handle is held as Box<dyn Trait> and each \
       constructor's concrete value is boxed where it is built. Without the \
       marker the handle is one concrete type (held by pointer in Go). \
       TypeScript ignores the marker." );
    ( "call",
      "The foreign symbol and the argument order it takes, e.g. call: \
       \"Load\"(service, region). Arguments are the extern's parameters, \
       literals, a foreign struct literal, or a nested foreign-symbol call, \
       e.g. \"FromFormula\"(expr, \"WithPrecision\"(precision)). Required in \
       every language block." );
    ( "yields",
      "Names what the call returns, position by position, so returns: can \
       project from it (e.g. yields: (cfg: go_config)). Also declares \
       positions outside the target's convention; the reserved '"
      ^ Ext_lib_vocab.error_sentinel
      ^ "' type marks the error position, at most once. Omit it when the \
         result already is the declared return type." );
    ( "returns",
      "Builds the extern's declared return type from the yields: names, as \
       every struct literal does: returns: app_config { endpoint: .cfg.Host }. \
       A field takes a reference or a match over one." );
    ( "errors",
      "Maps a foreign error sentinel to a declared error shape: errors: { \
       \"ErrBusy\" => overloaded }. An unmapped failure surfaces as the \
       generic contract error." );
    ( "sync",
      "The call blocks and is not awaited. Only meaningful in targets that \
       await extern calls by default (Rust, TypeScript); Go ignores it, since \
       every Go call is already synchronous." );
    ( "infallible",
      "The call returns a single value with no error position. Only meaningful \
       in Go, whose convention makes every call (value, error); the other \
       targets ignore it, since their error travels in the type or is thrown."
    );
    ( "ctx",
      "The call receives the target's own cancellation/deadline context in its \
       idiomatic position. Only valid on a foreign handle's own method call: \
       Go prepends ctx context.Context as the first parameter. In an op's own \
       'impl' body that is the operation's ctx; in a field source ('config: \
       cfg = .handle.method()') the constructor has no caller context, so Go \
       passes context.Background() (no deadline, no cancellation) for that \
       one-shot resolution. Rust and TypeScript have no equivalent convention \
       and ignore the marker." );
    ( "new",
      "The call constructs a TypeScript class with 'new' instead of calling a \
       plain function: new Symbol(args) instead of await Symbol(args). Only \
       meaningful in TypeScript; Go and Rust have no 'new' distinct from an \
       ordinary call and ignore the marker." );
    ( "variadic",
      "This logical parameter accepts a collection of values, not one: the \
       caller passes a list for it, and each language's own call: materializes \
       it in its own idiom (Go's opts ...Option spread, Rust's Vec<T>, \
       TypeScript's T[])." );
    ( Ext_lib_vocab.request_ref,
      Printf.sprintf
        "The canonical request, already assembled (method, path, headers, \
         body), read-only. Exists only as an argument to an extern call inside \
         %s; nowhere else, since there the request is not built yet (not in \
         construction, not in @http, @query, @timeout, or @retry)."
        (String.concat "/"
           (List.map (fun t -> "@" ^ t) Check_request_value.request_trait_names))
    );
  ]

(* The construct hover texts: the reserved words and the words the parser
   reads by position, one entry each. Checked against [Syntax_vocab] by
   [construct_docs_cover_the_parser_vocabulary], in both directions, so a
   construct the compiler gains hovers with something to say and one it loses
   stops being advertised. *)
let construct_docs : (string * string) list =
  [
    ( "struct",
      "A record shape: named members with types, serialized as an object." );
    ( "enum",
      "An open enumeration: strict on encode, lenient on decode (an unknown \
       value is carried, never a failure)." );
    ( "union",
      "A tagged sum: every variant carries a payload and travels internally \
       tagged by the discriminator field." );
    ( "op",
      "An operation: input and output shapes plus traits (transport, errors, \
       effect)." );
    ( "map",
      "A homogeneous map type, map[K]V. Keys that cannot be object keys can \
       escape to a pairs array with @entries." );
    ("pub", "Exports the declaration across module boundaries.");
    ("import", "Brings another module's declarations into dot-qualified scope.");
    ( "as",
      "Renames an import locally: import a.b as c makes the module reachable \
       as c." );
    ( "ext",
      "Two forms. 'ext <lib> { ... }' integrates a third-party library \
       declaratively: the module path per language, its foreign shapes, and \
       the extern functions and opaque handles that call into it. 'ext \
       contract|constraint|impl' declares a bespoke extension point bound per \
       language to a file#symbol reference." );
    ( "contract",
      "A bespoke function with a typed signature; emission is gated on a \
       conformance spec." );
    ("constraint", "A bespoke validation predicate attached at the boundary.");
    ( "impl",
      "After an operation's traits, 'impl .handle.method(args)' implements it \
       as a call into a declared opaque handle's method (the same call a field \
       can take as its value: 'config: cfg = .handle.method(args)'). After \
       'ext', it binds the operation it names to bespoke sources, taking that \
       operation's signature; add 'raw' to return an outcome the generated \
       glue decodes and discriminates." );
    ( "raw",
      "The bound symbol returns an outcome (success flag, code, body) and the \
       generated glue decodes it into the declared output or discriminates the \
       failure by its code." );
    ( "test",
      "A declared test, generated as a native test file per target (go test, \
       Vitest, cargo test): named bindings construct the entry, stub its \
       declared dependencies, call operations, and expect outcomes. A test \
       with every dependency stubbed is hermetic; one touching a real \
       dependency lands in the opt-in live suite." );
    ( "stub",
      "Substitutes a declared dependency of one operation on one client \
       binding: `.http` (from @http) answers with http.response, `.impl` (from \
       ext impl) answers with the operation's own types. The binding records \
       what crossed the dependency (`s.requests`)." );
    ( "expect",
      "Asserts a binding's outcome against a pattern: the output shape, a \
       declared error, or a tono.errors shape. `..` frees unnamed fields, \
       `any` asserts presence, `None` asserts absence." );
    ( "match",
      Printf.sprintf
        "A selection table over a field reference: literal patterns, one arm \
         each, exhaustive (a `_` wildcard covers the rest). Arms yield a \
         reference, a literal, or a stack of sources (%s). No nesting, no \
         comparators, no expressions. The subject may also be a map field \
         indexed by another field (`by_segment[.seg]`), which resolves to an \
         optional value; a `null` arm is then mandatory."
        (String.concat "/"
           (List.map (fun s -> "@" ^ s) Entry_scope.source_names)) );
    ( "null",
      "The mandatory absence arm on a match whose subject is optional (a \
       map-indexed field can always miss its key). Exactly one `null` arm is \
       required when the subject is `T?`, and none is allowed when it is not."
    );
  ]

let construct_doc (word : string) : string option =
  match List.assoc_opt word construct_docs with
  | Some doc -> Some doc
  | None -> List.assoc_opt word ext_lib_docs

(* The two forms of the map-indexed match that are not single reserved words,
   so they sit outside [construct_docs]'s bijection with [Syntax_vocab]: the
   indexed subject itself and "._" in an arm's value position. Resolved by
   AST span in Analysis, not by word lookup, since neither names a fixed
   token position (the subject is an entire [field_reference], and "_" is an
   ordinary identifier everywhere else it appears). *)
let match_form_docs : (string * string) list =
  [
    ( "map_index",
      "Indexes a map field by another in-scope field's value, resolving to the \
       map's value type wrapped optional (`T?`): the key may not be in the \
       map. The enclosing match's mandatory `null` arm is what handles that." );
    ( "subject_ref",
      "'._' refers back to the match subject, in a non-`null` arm's value \
       position, narrowed to the subject's underlying type (never optional \
       there: the `null` arm has already handled absence)." );
  ]

let match_form_doc (name : string) : string option =
  List.assoc_opt name match_form_docs

(* Primitive and marker hover: the wire decisions that most surprise SDK
   consumers belong right under the cursor. *)
let primitive_doc (p : string) : string option =
  match p with
  | "i64" ->
      Some
        "64-bit signed integer. Travels as a string on the wire: JSON numbers \
         lose precision past 2^53."
  | "u64" -> Some "64-bit unsigned integer. Travels as a string on the wire."
  | "i8" | "i16" | "i32" ->
      Some
        "Signed integer. Arithmetic wraps at the width (two's complement); \
         division truncates toward zero."
  | "u8" | "u16" | "u32" ->
      Some "Unsigned integer. Arithmetic wraps modulo 2^width."
  | "float" ->
      Some
        "IEEE 754 double. There is no decimal type: money is integer minor \
         units."
  | "bool" -> Some "Boolean."
  | "string" -> Some "UTF-8 text."
  | "bytes" -> Some "Binary payload, base64 on the wire."
  | "timestamp" -> Some "An instant in time, carried as a branded string."
  | "date" -> Some "A calendar date, carried as a branded string."
  | "duration" -> Some "A span of time, carried as a branded string."
  | "uuid" -> Some "A UUID, carried as a branded string."
  | _ -> None

let nullable_doc =
  "Two-state nullability: the value is present or absent, nothing else. Absent \
   and null collapse; the default encoding omits the field."
