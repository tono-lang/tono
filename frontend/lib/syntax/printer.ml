(* Pretty-printer for the surface AST. One canonical layout: declaration traits
   on their own lines above the keyword (except for ops, whose traits trail on
   the op line, where the grammar attaches them anyway), two-space indentation,
   one member, case, or variant per line, and a blank line between declarations.
   The printer assumes a diagnostic-free parse; [TError] gets a parseable
   placeholder only so a defensive caller never emits garbage. *)

(* Whether [needle] occurs in [hay]; used to keep a triple-quoted print from
   embedding its own delimiter. *)
let contains_substring hay needle =
  let hn = String.length hay and nn = String.length needle in
  let rec loop i =
    if i + nn > hn then false
    else if String.sub hay i nn = needle then true
    else loop (i + 1)
  in
  nn = 0 || loop 0

let escaped_string (s : string) : string =
  let b = Buffer.create (String.length s + 2) in
  Buffer.add_char b '"';
  String.iter
    (fun c ->
      match c with
      | '"' -> Buffer.add_string b "\\\""
      | '\\' -> Buffer.add_string b "\\\\"
      | '\n' -> Buffer.add_string b "\\n"
      | '\t' -> Buffer.add_string b "\\t"
      | '\r' -> Buffer.add_string b "\\r"
      | c -> Buffer.add_char b c)
    s;
  Buffer.add_char b '"';
  Buffer.contents b

let string_literal (s : string) : string =
  (* A multi-line string (a doc, typically) prints in triple-quote form so it
     stays readable and round-trips its newlines verbatim. The triple-quote lexer
     reads raw bytes, so this is only safe when the content cannot be mistaken for
     the closing delimiter: it must not contain a run of three double-quotes, and
     a trailing double-quote would merge with the closer. Anything else falls back
     to the always-valid escaped single-line form. *)
  let triple_safe =
    String.contains s '\n'
    && (not (contains_substring s "\"\"\""))
    && not (String.length s > 0 && s.[String.length s - 1] = '"')
  in
  if triple_safe then "\"\"\"" ^ s ^ "\"\"\"" else escaped_string s

(* Expand "1.5e+300" to positional notation: the literal grammar has no
   exponent form, so the digits are shifted around the decimal point. *)
let expand_exponent (s : string) : string =
  let e =
    match String.index_opt s 'e' with Some i -> i | None -> String.index s 'E'
  in
  let mant = String.sub s 0 e in
  let exp = int_of_string (String.sub s (e + 1) (String.length s - e - 1)) in
  let sign, mant =
    if mant.[0] = '-' then ("-", String.sub mant 1 (String.length mant - 1))
    else ("", mant)
  in
  let int_part, frac =
    match String.index_opt mant '.' with
    | Some d ->
        ( String.sub mant 0 d,
          String.sub mant (d + 1) (String.length mant - d - 1) )
    | None -> (mant, "")
  in
  let digits = int_part ^ frac in
  let point = String.length int_part + exp in
  let n = String.length digits in
  if point <= 0 then sign ^ "0." ^ String.make (-point) '0' ^ digits
  else if point >= n then sign ^ digits ^ String.make (point - n) '0' ^ ".0"
  else
    sign ^ String.sub digits 0 point ^ "." ^ String.sub digits point (n - point)

let float_literal (f : float) : string =
  if f <> f then "0.0" (* nan: no literal lexes to it, defensive only *)
  else if f = infinity || f = neg_infinity then
    (* An overflowing literal is the only positional spelling of infinity,
       mirroring how the lexer produced it. *)
    (if f < 0.0 then "-1" else "1") ^ String.make 309 '0' ^ ".0"
  else
    let s = ref (Printf.sprintf "%.17g" f) in
    (try
       for p = 1 to 17 do
         let c = Printf.sprintf "%.*g" p f in
         if float_of_string c = f then (
           s := c;
           raise Exit)
       done
     with Exit -> ());
    let s = !s in
    if String.contains s 'e' || String.contains s 'E' then expand_exponent s
    else if String.contains s '.' then s
    else s ^ ".0"

let rec print_ty (t : Ast.ty) : string =
  match t with
  | Ast.TPrim (p, _) -> p
  | Ast.TName (n, [], _) -> n
  | Ast.TName (n, args, _) ->
      n ^ "[" ^ String.concat ", " (List.map print_ty args) ^ "]"
  | Ast.TQName (q, n, [], _) -> q ^ "." ^ n
  | Ast.TQName (q, n, args, _) ->
      q ^ "." ^ n ^ "[" ^ String.concat ", " (List.map print_ty args) ^ "]"
  | Ast.TList (elem, _) -> "[]" ^ print_ty elem
  | Ast.TMap (k, v, _) -> "map[" ^ print_ty k ^ "]" ^ print_ty v
  | Ast.TNullable (inner, _) -> print_ty inner ^ "?"
  | Ast.TError _ -> "_"

let rec print_ref (r : Ast.ref_path) : string =
  let base = "." ^ String.concat "." r.Ast.segs in
  match r.Ast.index with
  | None -> base
  | Some idx -> base ^ "[" ^ print_ref idx ^ "]"

let rec print_trait_arg (a : Ast.trait_arg) : string =
  match a with
  | Ast.AString s -> string_literal s
  | Ast.AInt n -> string_of_int n
  | Ast.AFloat f -> float_literal f
  | Ast.AName n -> n
  | Ast.ARef r -> print_ref r
  | Ast.AKv (k, v) -> k ^ ": " ^ print_trait_arg v
  | Ast.AList xs -> "[" ^ String.concat ", " (List.map print_trait_arg xs) ^ "]"
  | Ast.ACtor c -> print_ctor_lit c
  | Ast.ACall ce -> print_call_expr ce

and print_ctor_lit (c : Ast.ctor_arg) : string =
  let fields =
    List.map (fun (n, _, v) -> n ^ ": " ^ print_trait_arg v) c.Ast.ctor_fields
  in
  c.Ast.ctor_name ^ " { " ^ String.concat ", " fields ^ " }"

and print_call_arg (a : Ast.call_arg) : string =
  match a with
  | Ast.CaParam (n, _) -> n
  | Ast.CaRef r -> print_ref r
  | Ast.CaCtor c -> print_ctor_lit c
  | Ast.CaLit (Ast.LStr s, _) -> string_literal s
  | Ast.CaLit (Ast.LInt n, _) -> string_of_int n
  | Ast.CaLit (Ast.LFloat f, _) -> float_literal f

and print_call_expr (ce : Ast.call_expr) : string =
  ce.Ast.ce_ns ^ "." ^ ce.Ast.ce_fn ^ "("
  ^ String.concat ", " (List.map print_call_arg ce.Ast.ce_args)
  ^ ")"

let print_trait (t : Ast.trait) : string =
  "@" ^ t.Ast.tname
  ^
  match t.Ast.targs with
  | [] -> ""
  | args -> "(" ^ String.concat ", " (List.map print_trait_arg args) ^ ")"

(* Traits appended to a line: members, cases, variants, and op declarations. *)
let trailing_traits (ts : Ast.trait list) : string =
  String.concat "" (List.map (fun t -> " " ^ print_trait t) ts)

let print_params = function [] -> "" | ps -> "[" ^ String.concat ", " ps ^ "]"

let print_pattern (p : Ast.match_pattern) : string =
  match p with
  | Ast.PString s -> string_literal s
  | Ast.PInt n -> string_of_int n
  | Ast.PName n -> n
  | Ast.PWildcard -> "_"
  | Ast.PNull -> "null"

let print_arm_value (v : Ast.arm_value) : string =
  match v with
  | Ast.AVRef r -> print_ref r
  | Ast.AVString s -> string_literal s
  | Ast.AVInt n -> string_of_int n
  | Ast.AVName n -> n
  | Ast.AVSubject _ -> "._"
  | Ast.AVSources ts -> String.concat " " (List.map (fun t -> print_trait t) ts)

(* [indent] is the indentation of the line the match opens on, so the arms and
   the closing brace stay anchored to the member however deep it sits. *)
let print_field_match ~indent (m : Ast.field_match) : string =
  let arm (a : Ast.match_arm) =
    indent ^ "  " ^ print_pattern a.Ast.pat ^ " => "
    ^ print_arm_value a.Ast.value
  in
  match m.Ast.arms with
  | [] -> "match " ^ print_ref m.Ast.subject ^ " {}"
  | arms ->
      "match " ^ print_ref m.Ast.subject ^ " {\n"
      ^ String.concat "\n" (List.map arm arms)
      ^ "\n" ^ indent ^ "}"

(* A handle method call, ".recv.method(args)": shared by an op's "impl" body
   and a member's value source. *)
let print_handle_call (hc : Ast.op_impl) : string =
  print_ref hc.Ast.oi_recv ^ "." ^ hc.Ast.oi_method ^ "("
  ^ String.concat ", " (List.map print_call_arg hc.Ast.oi_args)
  ^ ")"

let print_member (m : Ast.member) : string =
  "  " ^ m.Ast.mname ^ ": " ^ print_ty m.Ast.mtype
  ^ (match m.Ast.mvalue with
    | Some (Ast.MMatch fm) -> " = " ^ print_field_match ~indent:"  " fm
    | Some (Ast.MCall ce) -> " = " ^ print_call_expr ce
    | Some (Ast.MHandleCall hc) -> " = " ^ print_handle_call hc
    | None -> "")
  ^ trailing_traits m.Ast.mtraits

let print_enum_case (c : Ast.enum_case) : string =
  "  " ^ c.Ast.cname
  ^ (match c.Ast.cint with Some n -> " = " ^ string_of_int n | None -> "")
  ^ trailing_traits c.Ast.ctraits

let print_variant (v : Ast.union_variant) : string =
  "  " ^ v.Ast.vname
  ^ (match v.Ast.vpayload with Some t -> "(" ^ print_ty t ^ ")" | None -> "")
  ^ trailing_traits v.Ast.vtraits

(* header + a braced body, "{}" when empty *)
let braced (header : string) (lines : string list) : string =
  match lines with
  | [] -> header ^ " {}"
  | ls -> header ^ " {\n" ^ String.concat "\n" ls ^ "\n}"

(* The op form, shared by top-level ops and ops nested in a struct body (where
   [indent] is the body indentation). Op traits print one per line below the
   signature: an operation carries the whole protocol vocabulary (@http,
   @header, @timeout, @retry, @errors), which on one line runs past any usable
   width. They stay below, never above: whitespace is not significant, so a
   trait written above an op would bind to whatever was declared before it. *)
let print_op_impl ~indent (oi : Ast.op_impl) : string =
  "\n" ^ indent ^ "  impl " ^ print_handle_call oi

let print_op ~indent (d : Ast.decl) : string =
  match d.Ast.dkind with
  | Ast.DOp { pname; input; output; oimpl } ->
      let pub = if d.Ast.pub then "pub " else "" in
      let param =
        match (pname, input) with
        | Some n, Some t -> n ^ ": " ^ print_ty t
        | _, Some t -> print_ty t
        | _, None -> ""
      in
      let signature =
        indent ^ pub ^ "op " ^ d.Ast.dname ^ "(" ^ param ^ ")"
        ^ match output with Some t -> ": " ^ print_ty t | None -> ""
      in
      let traits =
        List.map (fun t -> "\n" ^ indent ^ "  " ^ print_trait t) d.Ast.dtraits
      in
      let impl =
        match oimpl with Some oi -> print_op_impl ~indent oi | None -> ""
      in
      signature ^ String.concat "" traits ^ impl
  | _ -> assert false

(* ── Test blocks ───────────────────────────────────────────────────────── *)

let is_bare_key (s : string) : bool =
  s <> ""
  && (match s.[0] with 'a' .. 'z' | 'A' .. 'Z' | '_' -> true | _ -> false)
  && String.for_all
       (function
         | 'a' .. 'z' | 'A' .. 'Z' | '0' .. '9' | '_' -> true | _ -> false)
       s

(* A map key prints bare when it lexes as an identifier, quoted otherwise
   (header names carry dashes). *)
let print_map_key (k : string) : string =
  if is_bare_key k then k else string_literal k

let print_value_head (h : Ast.value_head) : string =
  String.concat "." h.Ast.vh_segs

(* One canonical inline layout for values and patterns: a test reads as a
   script of one-line steps, and the fixture bodies are opaque strings. *)
let rec print_test_value (v : Ast.test_value) : string =
  match v with
  | Ast.TvStr (s, _) -> string_literal s
  | Ast.TvInt (n, _) -> string_of_int n
  | Ast.TvFloat (f, _) -> float_literal f
  | Ast.TvBool (b, _) -> if b then "true" else "false"
  | Ast.TvCtor c ->
      print_value_head c.tc_head ^ " " ^ print_ctor_body c.tc_fields
  | Ast.TvList (items, _) ->
      "[" ^ String.concat ", " (List.map print_test_value items) ^ "]"
  | Ast.TvMap ([], _) -> "{}"
  | Ast.TvMap (entries, _) ->
      "{ "
      ^ String.concat ", "
          (List.map
             (fun ((k, _), v) -> print_map_key k ^ ": " ^ print_test_value v)
             entries)
      ^ " }"
  | Ast.TvRef { base; path; _ } -> String.concat "." (base :: path)
  | Ast.TvError _ -> "_"

and print_ctor_body (fields : (string * Span.span * Ast.test_value) list) :
    string =
  match fields with
  | [] -> "{}"
  | fs ->
      "{ "
      ^ String.concat ", "
          (List.map (fun (n, _, v) -> n ^ ": " ^ print_test_value v) fs)
      ^ " }"

let rec print_test_pattern (p : Ast.test_pattern) : string =
  match p with
  | Ast.TpLit v -> print_test_value v
  | Ast.TpOk _ -> "ok"
  | Ast.TpCtor c ->
      print_value_head c.tp_head ^ " "
      ^ print_pattern_body
          (List.map (fun (n, _, f) -> (n, f)) c.tp_fields)
          c.tp_open
  | Ast.TpList (items, _) ->
      "[" ^ String.concat ", " (List.map print_test_pattern items) ^ "]"
  | Ast.TpMap { entries; map_open; _ } ->
      print_pattern_body
        (List.map (fun ((k, _), f) -> (print_map_key k, f)) entries)
        map_open
  | Ast.TpError _ -> "_"

(* The '..' mark prints last regardless of where it was written. *)
and print_pattern_body (fields : (string * Ast.test_pattern_field) list)
    (open_ : bool) : string =
  let entries =
    List.map (fun (n, f) -> n ^ ": " ^ print_pattern_field f) fields
    @ if open_ then [ ".." ] else []
  in
  match entries with [] -> "{}" | es -> "{ " ^ String.concat ", " es ^ " }"

and print_pattern_field (f : Ast.test_pattern_field) : string =
  match f with
  | Ast.TpfPat p -> print_test_pattern p
  | Ast.TpfAny _ -> "any"
  | Ast.TpfAbsent _ -> "None"

let print_test_item (i : Ast.test_item) : string =
  match i with
  | Ast.TiConstruct { bind; entry; fields; _ } ->
      "  " ^ bind ^ ": " ^ entry ^ " " ^ print_ctor_body fields
  | Ast.TiStub { bind; target; value; _ } ->
      let prefix = match bind with Some (b, _) -> b ^ ": " | None -> "" in
      "  " ^ prefix ^ "stub "
      ^ String.concat "." (List.map fst target.Ast.st_path)
      ^ ": " ^ print_test_value value
  | Ast.TiCall { bind; recv; op; input; _ } ->
      "  " ^ bind ^ ": " ^ recv ^ "." ^ op ^ "("
      ^ (match input with Some v -> print_test_value v | None -> "")
      ^ ")"
  | Ast.TiExpect { subject; requests; pattern; _ } ->
      "  expect " ^ subject
      ^ (if requests then ".requests" else "")
      ^ ": " ^ print_test_pattern pattern

(* ── FFI library blocks: ext <name> { ... } ──────────────────────────────
   Every nested block threads an explicit [~indent] prefix down to its own
   lines and children, the same discipline [print_field_match]/[print_op]
   already use. This is deliberate: reindenting an already-rendered string
   (splitting on '\n' and prepending) is NOT safe here, because a string
   literal's own value can itself contain a literal newline (the triple-quote
   form); blindly reindenting every line would inject whitespace into the
   middle of that string's content. *)

(* header + a braced body at [indent], "{}" when empty; [lines] are already
   fully prefixed with their own indent by the caller (mirrors [braced]). *)
let braced_at ~(indent : string) (header : string) (lines : string list) :
    string =
  match lines with
  | [] -> indent ^ header ^ " {}"
  | ls -> indent ^ header ^ " {\n" ^ String.concat "\n" ls ^ "\n" ^ indent ^ "}"

let print_lang_path ~indent (lp : Ast.lang_path) : string =
  indent ^ lp.Ast.lp_lang ^ ": " ^ escaped_string lp.Ast.lp_path

let print_foreign_field ~indent (f : Ast.foreign_field) : string =
  indent ^ f.Ast.ff_name ^ ": " ^ print_ty f.Ast.ff_type

let print_foreign_struct ~indent (s : Ast.foreign_struct) : string =
  braced_at ~indent
    ("struct " ^ s.Ast.fs_name)
    (List.map (print_foreign_field ~indent:(indent ^ "  ")) s.Ast.fs_fields)

let print_yields_ty (t : Ast.yields_ty) : string =
  match t with Ast.YType t -> print_ty t | Ast.YError _ -> "error"

let print_yields ~indent (ys : Ast.yields_pos list) : string =
  indent ^ "yields: ("
  ^ String.concat ", "
      (List.map
         (fun (y : Ast.yields_pos) ->
           y.yp_name ^ ": " ^ print_yields_ty y.yp_ty)
         ys)
  ^ ")"

let print_returns_value ~indent (v : Ast.returns_value) : string =
  match v with
  | Ast.RvRef r -> print_ref r
  | Ast.RvMatch fm -> print_field_match ~indent fm

let print_returns_field ~indent (f : Ast.returns_field) : string =
  indent ^ f.Ast.rf_name ^ ": " ^ print_returns_value ~indent f.Ast.rf_value

let print_returns ~indent (r : Ast.returns_lit) : string =
  braced_at ~indent
    ("returns: " ^ print_ty r.Ast.rl_type)
    (List.map (print_returns_field ~indent:(indent ^ "  ")) r.Ast.rl_fields)

let print_errors ~indent (es : Ast.error_map_entry list) : string =
  braced_at ~indent "errors:"
    (List.map
       (fun (e : Ast.error_map_entry) ->
         indent ^ "  " ^ escaped_string e.em_sentinel ^ " => " ^ e.em_type)
       es)

let print_call ~indent (symbol : string) (args : Ast.call_arg list) : string =
  indent ^ "call: " ^ escaped_string symbol ^ "("
  ^ String.concat ", " (List.map print_call_arg args)
  ^ ")"

let print_extern_lang_body ~indent (b : Ast.extern_lang_body) : string =
  let inner = indent ^ "  " in
  let call_line =
    [ print_call ~indent:inner b.Ast.elb_call_symbol b.elb_call_args ]
  in
  let yields_line =
    match b.Ast.elb_yields with
    | Some ys -> [ print_yields ~indent:inner ys ]
    | None -> []
  in
  let returns_lines =
    match b.Ast.elb_returns with
    | Some r -> [ print_returns ~indent:inner r ]
    | None -> []
  in
  let errors_lines =
    if b.Ast.elb_errors = [] then []
    else [ print_errors ~indent:inner b.elb_errors ]
  in
  let sync_lines = if b.Ast.elb_sync then [ inner ^ "sync" ] else [] in
  let infallible_lines =
    if b.Ast.elb_infallible then [ inner ^ "infallible" ] else []
  in
  let ctx_lines = if b.Ast.elb_ctx then [ inner ^ "ctx" ] else [] in
  braced_at ~indent b.Ast.elb_lang
    (call_line @ sync_lines @ infallible_lines @ ctx_lines @ yields_line
   @ returns_lines @ errors_lines)

let print_extern ~indent (e : Ast.extern_decl) : string =
  let inner = indent ^ "  " in
  let params =
    String.concat ", "
      (List.map
         (fun (p : Ast.extern_param) -> p.ep_name ^ ": " ^ print_ty p.ep_type)
         e.Ast.ed_params)
  in
  let header =
    "extern " ^ e.Ast.ed_name ^ "(" ^ params ^ "): " ^ print_ty e.ed_return
  in
  braced_at ~indent header
    (List.map (print_extern_lang_body ~indent:inner) e.Ast.ed_langs)

let print_opaque_type ~indent (t : Ast.opaque_type) : string =
  let inner = indent ^ "  " in
  let instance =
    match t.Ast.opq_instance with
    | None -> ""
    | Some i ->
        "("
        ^ escaped_string i.Ast.oi_foreign_name
        ^ ", " ^ print_ty i.Ast.oi_arg ^ ")"
  in
  braced_at ~indent
    ("type " ^ t.Ast.opq_name ^ instance)
    (List.map (print_extern ~indent:inner) t.Ast.opq_methods)

(* Sections (lang paths, structs, opaque types, free externs) are separated
   by a blank line whenever a later section is non-empty; whitespace is
   insignificant to the grammar, so this only needs to be deterministic. *)
let print_ext_lib_body ~indent (b : Ast.ext_lib_body) : string list =
  let section lines = if lines = [] then [] else "" :: lines in
  List.map (print_lang_path ~indent) b.Ast.elib_langs
  @ section (List.map (print_foreign_struct ~indent) b.elib_structs)
  @ section (List.map (print_opaque_type ~indent) b.elib_types)
  @ section (List.map (print_extern ~indent) b.elib_externs)

let print_decl (d : Ast.decl) : string =
  let pub = if d.Ast.pub then "pub " else "" in
  match d.Ast.dkind with
  | Ast.DOp _ ->
      (* Op traits print trailing: whitespace is not significant, so any trait
         between an op and the next declaration binds to the op regardless. *)
      print_op ~indent:"" d
  | kind ->
      let above =
        String.concat ""
          (List.map (fun t -> print_trait t ^ "\n") d.Ast.dtraits)
      in
      let body =
        match kind with
        | Ast.DStruct { params; members; ops } ->
            (* An entry prints its ops after the fields, separated by a blank
               line so the construction surface reads apart from the methods. *)
            let member_lines = List.map print_member members in
            (* Ops are blocks (signature plus a trait per line), so they are
               separated from each other the same way declarations are. *)
            let op_lines =
              match List.map (print_op ~indent:"  ") ops with
              | [] -> []
              | first :: rest ->
                  first :: List.concat_map (fun o -> [ ""; o ]) rest
            in
            let lines =
              match (member_lines, op_lines) with
              | ms, [] -> ms
              | [], os -> os
              | ms, os -> ms @ ("" :: os)
            in
            braced (pub ^ "struct " ^ d.Ast.dname ^ print_params params) lines
        | Ast.DEnum { cases } ->
            braced
              (pub ^ "enum " ^ d.Ast.dname)
              (List.map print_enum_case cases)
        | Ast.DUnion { params; variants } ->
            braced
              (pub ^ "union " ^ d.Ast.dname ^ print_params params)
              (List.map print_variant variants)
        | Ast.DExt { ekind; esig; eraw; ebindings; econformance; _ } ->
            let kw =
              match ekind with
              | Ast.EHook -> "hook"
              | Ast.EContract -> "contract"
              | Ast.EConstraint -> "constraint"
              | Ast.EImpl -> "impl"
            in
            let raw = match eraw with Some _ -> " raw" | None -> "" in
            let signature =
              match esig with
              | Some { Ast.esig_in; esig_out } ->
                  " (" ^ print_ty esig_in ^ ") -> " ^ print_ty esig_out
              | None -> ""
            in
            (* Binding targets are file references, which on some platforms
               carry characters the literal grammar has to escape. *)
            let entry key value = "  " ^ key ^ ": " ^ escaped_string value in
            let lines =
              List.map
                (fun (b : Ast.ext_binding) -> entry b.lang b.target)
                ebindings
              @
              match econformance with
              | Some c -> [ entry "conformance" c ]
              | None -> []
            in
            braced
              (pub ^ "ext " ^ kw ^ " " ^ d.Ast.dname ^ raw ^ signature)
              lines
        | Ast.DExtLib { body; _ } ->
            braced
              (pub ^ "ext " ^ d.Ast.dname)
              (print_ext_lib_body ~indent:"  " body)
        | Ast.DTest { titems } ->
            braced
              ("test " ^ string_literal d.Ast.dname)
              (List.map print_test_item titems)
        | Ast.DOp _ -> assert false
      in
      above ^ body

let print_import (i : Ast.import) : string =
  "import "
  ^ String.concat "." i.Ast.imported_path
  ^ match i.Ast.alias with Some a -> " as " ^ a | None -> ""

(* Imports print first as a block (one per line), then a blank line, then the
   declarations. An import-only or declaration-only file omits the separator. *)
let print_file (f : Ast.file) : string =
  let imports =
    match f.Ast.imports with
    | [] -> ""
    | is -> String.concat "\n" (List.map print_import is) ^ "\n"
  in
  let decls =
    match f.Ast.decls with
    | [] -> ""
    | ds -> String.concat "\n\n" (List.map print_decl ds) ^ "\n"
  in
  match (f.Ast.imports, f.Ast.decls) with
  | [], _ -> decls
  | _, [] -> imports
  | _ -> imports ^ "\n" ^ decls
