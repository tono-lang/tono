(* What the editor *says*: the trait contracts, construct prose, and primitive
   notes behind hover, completion and signature help. Kept apart from
   [Analysis], which decides *where* the cursor is; this module holds no
   position logic and never touches a span.

   Every enumerated vocabulary here is rendered from the checker that enforces
   it (hook slots, the @str:: catalog, the value sources), so the editor can
   never explain a form the compiler does not accept. *)

module Ast = Tono_frontend.Ast
module Check_ext = Tono_frontend.Check_ext
module Check_entries = Tono_frontend.Check_entries
module Entry_scope = Tono_frontend.Entry_scope

(* The trait contracts surfaced on hover, offered after `@`, and expanded by
   signature help: one table, three consumers, so the documented keys can
   never drift between them. The frontend has no central trait registry (each
   checker pattern-matches its own keys); keep entries in step with the
   checkers that read them (check_http, check_constraints, protocol_http). *)
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
          "The value matched against the payload's code field to select this \
           error shape.";
        ti_keys = [ ("value", "string") ];
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
    ( "httpLabel",
      {
        ti_doc =
          "Binds the member to the matching {placeholder} in the operation's \
           @http path. A nullable member cannot fill one.";
        ti_keys = [];
      } );
    ( "httpQuery",
      {
        ti_doc = "Binds the member to a query string parameter.";
        ti_keys = [ ("", "string") ];
      } );
    ( "httpHeader",
      {
        ti_doc = "Binds the member to a request header.";
        ti_keys = [ ("", "string") ];
      } );
    ( "httpPayload",
      {
        ti_doc =
          "The member is the whole request or response body, instead of one \
           field within it. At most one per operation, and never alongside \
           unmarked body members.";
        ti_keys = [];
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

(* Construct hover texts. The hook slot list is rendered from the exact table
   the typechecker enforces (Check_ext.hook_slots): the enumerated semantics
   must never grow a second hand-written copy. *)
let construct_doc (word : string) : string option =
  match word with
  | "struct" ->
      Some "A record shape: named members with types, serialized as an object."
  | "enum" ->
      Some
        "An open enumeration: strict on encode, lenient on decode (an unknown \
         value is carried, never a failure)."
  | "union" ->
      Some
        "A tagged sum: every variant carries a payload and travels internally \
         tagged by the discriminator field."
  | "op" ->
      Some
        "An operation: input and output shapes plus traits (transport, errors, \
         effect)."
  | "map" ->
      Some
        "A homogeneous map type, map[K]V. Keys that cannot be object keys can \
         escape to a pairs array with @entries."
  | "pub" -> Some "Exports the declaration across module boundaries."
  | "import" ->
      Some "Brings another module's declarations into dot-qualified scope."
  | "ext" ->
      Some
        "A bespoke extension point (hook, contract, constraint, or impl), \
         bound per language to a file#symbol reference."
  | "hook" ->
      Some
        (Printf.sprintf
           "Fills a fixed lifecycle slot (%s), bound per language to a \
            file#symbol reference. Hooks take no signature."
           (String.concat ", " Check_ext.hook_slots))
  | "contract" ->
      Some
        "A bespoke function with a typed signature; emission is gated on a \
         conformance spec."
  | "constraint" ->
      Some "A bespoke validation predicate attached at the boundary."
  | "impl" ->
      Some
        "Implements the operation it names with bespoke sources, taking that \
         operation's signature. Add 'raw' to return an outcome the generated \
         glue decodes and discriminates."
  | "raw" ->
      Some
        "The bound symbol returns an outcome (success flag, code, body) and \
         the generated glue decodes it into the declared output or \
         discriminates the failure by its code."
  | "test" ->
      Some
        "A declared test, generated as a native test file per target (go test, \
         Vitest, cargo test): named bindings construct the entry, stub its \
         declared dependencies, call operations, and expect outcomes. A test \
         with every dependency stubbed is hermetic; one touching a real \
         dependency lands in the opt-in live suite."
  | "stub" ->
      Some
        "Substitutes a declared dependency of one operation on one client \
         binding: `.http` (from @http) answers with http.response, `.impl` \
         (from ext impl) answers with the operation's own types. The binding \
         records what crossed the dependency (`s.requests`)."
  | "expect" ->
      Some
        "Asserts a binding's outcome against a pattern: the output shape, a \
         declared error, or a tono.errors shape. `..` frees unnamed fields, \
         `any` asserts presence, `None` asserts absence."
  | "match" ->
      Some
        (Printf.sprintf
           "A selection table over a field reference: literal patterns, one \
            arm each, exhaustive (a `_` wildcard covers the rest). Arms yield \
            a reference, a literal, or a stack of sources (%s). No nesting, no \
            comparators, no expressions."
           (String.concat "/"
              (List.map (fun s -> "@" ^ s) Entry_scope.source_names)))
  | _ -> None

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
