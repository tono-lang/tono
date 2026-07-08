; Syntax highlighting for tono.

; Keywords
[
  "struct"
  "enum"
  "union"
  "op"
  "pub"
  "map"
  "ext"
  "hook"
  "contract"
  "constraint"
  "import"
  "as"
] @keyword

; Declaration names
(struct name: (identifier) @type.definition)
(enum name: (identifier) @type.definition)
(union name: (identifier) @type.definition)
(operation name: (identifier) @function)
(extension name: (identifier) @function)
(ext_binding key: (identifier) @property)
(import path: (module_path (identifier) @module))

; Members and cases
(member name: (identifier) @property)
(enum_case name: (identifier) @constant)
(variant name: (identifier) @constructor)

; Types
(primitive_type) @type.builtin
(named_type (identifier) @type)
(qualified_type module: (identifier) @module)
(qualified_type name: (identifier) @type)

; Traits (annotations)
(trait "@" @attribute)
(trait name: (identifier) @attribute)
(trait_pair key: (identifier) @property)

; Literals
(string) @string
(number) @number
(comment) @comment

; Punctuation
[ "{" "}" "[" "]" "(" ")" ] @punctuation.bracket
[ ":" "," "?" "=" ] @punctuation.delimiter
