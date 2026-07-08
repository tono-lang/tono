/**
 * Tree-sitter grammar for the tono interface language.
 *
 * This is the highlight grammar (a second, lightweight parser kept in sync with
 * the OCaml frontend): it exists for editor and GitHub syntax highlighting, not
 * for typechecking. The OCaml recursive-descent parser remains the single source
 * of truth for parsing and diagnostics; this grammar only needs to color a file
 * and tolerate work in progress.
 */

const PRIMITIVES = [
  "bool", "string", "bytes", "float", "timestamp", "date", "duration", "uuid",
  "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64",
];

function commaSep1(rule) {
  return seq(rule, repeat(seq(",", rule)), optional(","));
}
function commaSep(rule) {
  return optional(commaSep1(rule));
}

module.exports = grammar({
  name: "tono",

  word: ($) => $.identifier,
  extras: ($) => [/\s/, $.comment],

  rules: {
    source_file: ($) => repeat(choice($.import, $._declaration)),

    comment: ($) => token(seq("//", /.*/)),

    import: ($) =>
      seq(
        "import",
        field("path", $.module_path),
        optional(seq("as", field("alias", $.identifier))),
      ),

    module_path: ($) => seq($.identifier, repeat(seq(".", $.identifier))),

    // A declaration may be preceded by trait annotations and an optional `pub`.
    _declaration: ($) =>
      seq(
        repeat($.trait),
        optional("pub"),
        choice($.struct, $.enum, $.union, $.operation, $.extension),
      ),

    struct: ($) =>
      seq(
        "struct",
        field("name", $.identifier),
        optional($.type_parameters),
        "{",
        repeat($.member),
        "}",
      ),

    // Generic parameters on a struct or union head: `box[T, U]`.
    type_parameters: ($) => seq("[", commaSep1($.identifier), "]"),

    member: ($) =>
      seq(
        field("name", $.identifier),
        ":",
        field("type", $._type),
        repeat($.trait),
        optional(","),
      ),

    enum: ($) =>
      seq("enum", field("name", $.identifier), "{", commaSep($.enum_case), "}"),

    enum_case: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("=", field("value", $.number))),
        repeat($.trait),
      ),

    union: ($) =>
      seq(
        "union",
        field("name", $.identifier),
        optional($.type_parameters),
        "{",
        commaSep($.variant),
        "}",
      ),

    variant: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("(", field("payload", $._type), ")")),
        repeat($.trait),
      ),

    // Trailing traits bind to the operation, not to the next declaration (the
    // OCaml parser makes the same choice); prec.right prefers extending the op.
    operation: ($) =>
      prec.right(
        seq(
          "op",
          field("name", $.identifier),
          "(",
          optional(field("input", $._type)),
          ")",
          optional(seq(":", field("output", $._type))),
          repeat($.trait),
        ),
      ),

    // `ext (hook|contract|constraint) name [(in) -> out] { lang: "file#symbol" }`,
    // the bespoke escape hatch; bindings map a language tag (or the reserved
    // `conformance` key) to a string reference.
    extension: ($) =>
      seq(
        "ext",
        field("kind", choice("hook", "contract", "constraint")),
        field("name", $.identifier),
        optional($.ext_signature),
        "{",
        commaSep($.ext_binding),
        "}",
      ),

    ext_signature: ($) => seq("(", $._type, ")", "->", $._type),

    ext_binding: ($) =>
      seq(field("key", $.identifier), ":", field("value", $.string)),

    // `@name` or `@name(arg, key: value, ...)`.
    trait: ($) =>
      seq("@", field("name", $.identifier), optional($.trait_arguments)),

    trait_arguments: ($) => seq("(", commaSep($.trait_argument), ")"),

    trait_argument: ($) => choice($.trait_pair, $._trait_value),

    trait_pair: ($) =>
      seq(field("key", $.identifier), ":", field("value", $._trait_value)),

    _trait_value: ($) => choice($.string, $.number, $.identifier),

    // Nullability is a trailing `?` on any base type, so it never left-recurses.
    _type: ($) => choice($._base_type, $.nullable_type),

    // Higher precedence than a bare base type so a trailing `?` is consumed here
    // (shift) rather than ending the type (reduce).
    nullable_type: ($) => prec(1, seq($._base_type, "?")),

    _base_type: ($) =>
      choice(
        $.primitive_type,
        $.named_type,
        $.qualified_type,
        $.list_type,
        $.map_type,
      ),

    primitive_type: ($) => choice(...PRIMITIVES),

    named_type: ($) => seq($.identifier, optional($.type_arguments)),

    // A cross-module reference: `payments.charge`.
    qualified_type: ($) =>
      seq(
        field("module", $.identifier),
        ".",
        field("name", $.identifier),
        optional($.type_arguments),
      ),

    type_arguments: ($) => seq("[", commaSep1($._type), "]"),

    list_type: ($) => seq("[", "]", $._type),

    map_type: ($) => seq("map", "[", $._type, "]", $._type),

    string: ($) =>
      token(seq('"', repeat(choice(/[^"\\]/, /\\./)), '"')),

    number: ($) => /-?\d+(\.\d+)?/,

    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,
  },
});
