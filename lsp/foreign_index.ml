(* The index `tono index` writes, as the editor reads it. The file is the
   whole contract between the two: one JSON per (ext, language) with the
   key it was built from and the library's symbols in a neutral shape (a
   name, a kind, its signatures, its members). A language the editor has
   never heard of works the moment an extractor writes this shape.

   The index is a suggestion, never a verdict. What it lists may still fail
   the binding check, and what it misses is a suggestion that does not
   appear; neither can make a wrong spelling pass. That is why an unknown
   kind is kept rather than refused, and why a key that does not match the
   project as it is now makes the whole file unusable: a stale suggestion
   is worse than none. *)

open Lsp.Types

type kind =
  | Function
  | Class
  | Struct
  | Interface
  | Type
  | Enum
  | Const
  | Namespace
  | Trait
  | Other

type member_kind =
  | Method
  | Field
  | Constructor
  | Member_function
  | Member_type
  | Member_const
  | Member_other

type member = {
  mname : string;
  mkind : member_kind;
  static : bool;
  msignatures : string list;
}

type symbol = {
  name : string;
  kind : kind;
  signatures : string list;
  doc : string;
  members : member list;
}

type key = {
  ext : string;
  lang : string;
  package : string;
  version : string;
  lockfile_path : string;
  lockfile_digest : string;
  format : int;
}

type t = { key : key; note : string option; symbols : symbol list }
type status = Ready of t | Missing of string
type lookup = ext:string -> lang:string -> status

let format = 1
let no_lockfile = "none"

(* --- reading --- *)

let kind_of_string = function
  | "function" -> Function
  | "class" -> Class
  | "struct" -> Struct
  | "interface" -> Interface
  | "type" -> Type
  | "enum" -> Enum
  | "const" -> Const
  | "namespace" -> Namespace
  | "trait" -> Trait
  | _ -> Other

let member_kind_of_string = function
  | "method" -> Method
  | "field" -> Field
  | "constructor" -> Constructor
  | "function" -> Member_function
  | "type" -> Member_type
  | "const" -> Member_const
  | _ -> Member_other

let member_of_json (j : Yojson.Safe.t) : member =
  let open Yojson.Safe.Util in
  {
    mname = to_string (member "name" j);
    mkind = member_kind_of_string (to_string (member "kind" j));
    static = (match member "static" j with `Bool b -> b | _ -> false);
    msignatures =
      (match member "signatures" j with
      | `List l -> List.filter_map to_string_option l
      | _ -> []);
  }

let symbol_of_json (j : Yojson.Safe.t) : symbol =
  let open Yojson.Safe.Util in
  {
    name = to_string (member "name" j);
    kind = kind_of_string (to_string (member "kind" j));
    signatures =
      (match member "signatures" j with
      | `List l -> List.filter_map to_string_option l
      | _ -> []);
    doc = (match member "doc" j with `String s -> s | _ -> "");
    members =
      (match member "members" j with
      | `List l -> List.map member_of_json l
      | _ -> []);
  }

let key_of_json (j : Yojson.Safe.t) : key =
  let open Yojson.Safe.Util in
  let lock = member "lockfile" j in
  {
    ext = to_string (member "ext" j);
    lang = to_string (member "lang" j);
    package = to_string (member "package" j);
    version = to_string (member "version" j);
    lockfile_path = to_string (member "path" lock);
    lockfile_digest = to_string (member "digest" lock);
    format = to_int (member "format" j);
  }

let of_json (j : Yojson.Safe.t) : (t, string) result =
  let open Yojson.Safe.Util in
  try
    match to_int (member "tono_index_version" j) with
    | v when v <> format ->
        Error
          (Printf.sprintf "index format %d, this editor reads %d (rebuild it)" v
             format)
    | _ ->
        Ok
          {
            key = key_of_json (member "key" j);
            note =
              (match member "note" j with `String s -> Some s | _ -> None);
            symbols =
              (match member "symbols" j with
              | `List l -> List.map symbol_of_json l
              | _ -> []);
          }
  with Type_error (msg, _) -> Error ("index unreadable: " ^ msg)

let of_string (text : string) : (t, string) result =
  match Yojson.Safe.from_string text with
  | j -> of_json j
  | exception Yojson.Json_error msg -> Error ("index unreadable: " ^ msg)

(* --- the key --- *)

(* The same few lines as the builder's: a digest both sides agree on
   without a library either would have to add. Not a security primitive. *)
let fnv1a64_hex (s : string) : string =
  let h = ref 0xcbf29ce484222325L in
  String.iter
    (fun c ->
      h := Int64.logxor !h (Int64.of_int (Char.code c));
      h := Int64.mul !h 0x100000001b3L)
    s;
  Printf.sprintf "%016Lx" !h

let expected_key ~ext ~lang ~package ~version ~lockfile_path ~lockfile : key =
  {
    ext;
    lang;
    package;
    version;
    lockfile_path;
    lockfile_digest =
      (match lockfile with
      | Some bytes -> fnv1a64_hex bytes
      | None -> no_lockfile);
    format;
  }

let key_matches (t : t) (k : key) : bool = t.key = k

(* --- completion --- *)

type position =
  | Call_head of { after_new : bool }
  | Type_pos
  | Function_pos
  | Path
  | Member of { head : string; base : position }

let symbol (t : t) (name : string) : symbol option =
  List.find_opt (fun s -> s.name = name) t.symbols

let item_kind = function
  | Function -> CompletionItemKind.Function
  | Class -> CompletionItemKind.Class
  | Struct -> CompletionItemKind.Struct
  | Interface | Trait -> CompletionItemKind.Interface
  | Type -> CompletionItemKind.TypeParameter
  | Enum -> CompletionItemKind.Enum
  | Const -> CompletionItemKind.Constant
  | Namespace -> CompletionItemKind.Module
  | Other -> CompletionItemKind.Text

let member_item_kind = function
  | Method -> CompletionItemKind.Method
  | Field -> CompletionItemKind.Field
  | Constructor -> CompletionItemKind.Constructor
  | Member_function -> CompletionItemKind.Function
  | Member_type -> CompletionItemKind.TypeParameter
  | Member_const -> CompletionItemKind.EnumMember
  | Member_other -> CompletionItemKind.Text

let kind_word = function
  | Function -> "function"
  | Class -> "class"
  | Struct -> "struct"
  | Interface -> "interface"
  | Type -> "type"
  | Enum -> "enum"
  | Const -> "const"
  | Namespace -> "namespace"
  | Trait -> "trait"
  | Other -> "symbol"

(* The detail line: the first signature, with how many more there are, so
   an overloaded name stays one item. *)
let detail_of ~(word : string) (signatures : string list) : string =
  match signatures with
  | [] -> word
  | [ s ] -> s
  | s :: rest -> Printf.sprintf "%s (+%d overloads)" s (List.length rest)

let symbol_item ?label (s : symbol) : CompletionItem.t =
  let documentation = if s.doc = "" then None else Some (`String s.doc) in
  CompletionItem.create
    ~label:(Option.value ~default:s.name label)
    ~kind:(item_kind s.kind)
    ~detail:(detail_of ~word:(kind_word s.kind) s.signatures)
    ?documentation ()

let member_item (m : member) : CompletionItem.t =
  let detail = detail_of ~word:"member" m.msignatures in
  CompletionItem.create ~label:m.mname ~kind:(member_item_kind m.mkind)
    ~detail:(if m.static then "static " ^ detail else detail)
    ()

(* Which kinds a position offers. An unknown kind is offered everywhere:
   the index never decides, and a wrong item is caught by the check. *)
let offered (p : position) (k : kind) : bool =
  match (p, k) with
  | _, Other -> true
  | Call_head { after_new = true }, Class -> true
  | Call_head { after_new = true }, _ -> false
  | Call_head _, (Function | Class | Struct | Namespace) -> true
  | Call_head _, _ -> false
  | Type_pos, (Class | Struct | Interface | Type | Enum | Trait | Namespace) ->
      true
  | Type_pos, _ -> false
  | Function_pos, (Function | Namespace) -> true
  | Function_pos, _ -> false
  | Path, _ -> false
  | Member _, _ -> true

(* The one-segment tail of [name] after [head] and a separator, if [name]
   is spelled as a member of [head] (a Rust module path, a nested
   namespace). *)
let tail_under ~(head : string) (name : string) : string option =
  let n = String.length name and h = String.length head in
  let after sep =
    let l = String.length sep in
    if n > h + l && String.sub name 0 h = head && String.sub name h l = sep then
      let tail = String.sub name (h + l) (n - h - l) in
      if String.contains tail '.' then None
      else
        match String.index_opt tail ':' with
        | Some _ -> None
        | None -> Some tail
    else None
  in
  match after "::" with Some t -> Some t | None -> after "."

let items (t : t) (p : position) : CompletionItem.t list =
  match p with
  | Member { head; _ } -> (
      let nested =
        List.filter_map
          (fun s ->
            Option.map
              (fun label -> symbol_item ~label s)
              (tail_under ~head s.name))
          t.symbols
      in
      match symbol t head with
      | Some s -> List.map member_item s.members @ nested
      | None -> nested)
  | _ ->
      List.filter_map
        (fun s -> if offered p s.kind then Some (symbol_item s) else None)
        t.symbols

let describe = function
  | Ready t -> (
      let base =
        Printf.sprintf
          "%d symbols of %s %s indexed for completion inside #(...)"
          (List.length t.symbols) t.key.package t.key.version
      in
      match t.note with Some n -> base ^ "\n\n" ^ n | None -> base)
  | Missing reason -> "no completion inside #(...): " ^ reason
