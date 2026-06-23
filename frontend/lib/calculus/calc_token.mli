(* Lexical tokens for the calculus: expression operators, calculus keywords, and
   integer literals kept as text to preserve i64. *)

type kind =
  | KwFn
  | KwIf
  | KwThen
  | KwElse
  | KwLet
  | KwIn
  | KwMatch
  | KwMap
  | KwFilter
  | KwFold
  | KwTrue
  | KwFalse
  | KwSome
  | KwNone
  | KwUnknown
  | Prim of string
  | Ident of string
  | Int of string
  | Float of float
  | Str of string
  | Plus
  | Minus
  | Star
  | Slash
  | Percent
  | EqEq
  | BangEq
  | Lt
  | Gt
  | LtEq
  | GtEq
  | AmpAmp
  | PipePipe
  | Bang
  | PlusPlus
  | QQuestion
  | Arrow
  | FatArrow
  | Eq
  | LParen
  | RParen
  | LBrace
  | RBrace
  | LBracket
  | RBracket
  | Colon
  | Comma
  | Dot
  | Question
  | Eof

type t = { kind : kind; span : Span.span }

(* A human label for a token kind, for diagnostic messages. *)
val describe : kind -> string
