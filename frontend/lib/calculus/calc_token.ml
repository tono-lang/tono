(* Lexical tokens for the calculus. Distinct from the .tono token set: the
   calculus has expression operators, its own keywords, and keeps integer
   literals as text to preserve i64 without host precision loss. *)

type kind =
  (* keywords *)
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
  (* atoms *)
  | Prim of string (* a primitive type keyword, e.g. "i64" *)
  | Ident of string
  | Int of string (* raw digits (with optional '-' folded by the parser) *)
  | Float of float
  | Str of string
  (* operators *)
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
  | QQuestion (* ?? coalesce *)
  | Arrow (* -> *)
  | FatArrow (* => *)
  | Eq (* = *)
  (* punctuation *)
  | LParen
  | RParen
  | LBrace
  | RBrace
  | LBracket
  | RBracket
  | Colon
  | Comma
  | Dot
  | Question (* ? for nullable types *)
  | Eof

type t = { kind : kind; span : Span.span }

let describe : kind -> string = function
  | KwFn -> "'fn'"
  | KwIf -> "'if'"
  | KwThen -> "'then'"
  | KwElse -> "'else'"
  | KwLet -> "'let'"
  | KwIn -> "'in'"
  | KwMatch -> "'match'"
  | KwMap -> "'map'"
  | KwFilter -> "'filter'"
  | KwFold -> "'fold'"
  | KwTrue -> "'true'"
  | KwFalse -> "'false'"
  | KwSome -> "'Some'"
  | KwNone -> "'None'"
  | KwUnknown -> "'Unknown'"
  | Prim s -> Printf.sprintf "type '%s'" s
  | Ident s -> Printf.sprintf "identifier '%s'" s
  | Int s -> Printf.sprintf "integer '%s'" s
  | Float _ -> "float literal"
  | Str _ -> "string literal"
  | Plus -> "'+'"
  | Minus -> "'-'"
  | Star -> "'*'"
  | Slash -> "'/'"
  | Percent -> "'%'"
  | EqEq -> "'=='"
  | BangEq -> "'!='"
  | Lt -> "'<'"
  | Gt -> "'>'"
  | LtEq -> "'<='"
  | GtEq -> "'>='"
  | AmpAmp -> "'&&'"
  | PipePipe -> "'||'"
  | Bang -> "'!'"
  | PlusPlus -> "'++'"
  | QQuestion -> "'??'"
  | Arrow -> "'->'"
  | FatArrow -> "'=>'"
  | Eq -> "'='"
  | LParen -> "'('"
  | RParen -> "')'"
  | LBrace -> "'{'"
  | RBrace -> "'}'"
  | LBracket -> "'['"
  | RBracket -> "']'"
  | Colon -> "':'"
  | Comma -> "','"
  | Dot -> "'.'"
  | Question -> "'?'"
  | Eof -> "end of input"
