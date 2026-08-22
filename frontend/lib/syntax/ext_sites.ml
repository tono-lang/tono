(* See ext_sites.mli. *)

type kind = Path | Handle | Struct | Op | Method

type site = {
  ext : string;
  lang : string;
  kind : kind;
  owner : string option;
  name : string option;
  span : Span.span;
}

let kind_to_string = function
  | Path -> "path"
  | Handle -> "handle"
  | Struct -> "struct"
  | Op -> "op"
  | Method -> "method"

let call_arg_span (a : Ast.call_arg) : Span.span =
  match a with
  | Ast.CaParam (_, s)
  | Ast.CaParamAs (_, _, _, s)
  | Ast.CaLit (_, s)
  | Ast.CaList (_, s)
  | Ast.CaForeign (_, s) ->
      s
  | Ast.CaRef r -> r.ref_span
  | Ast.CaCtor c -> c.ctor_span
  | Ast.CaCall nc -> nc.nc_span

(* The [call:] line is what a target-side finding is attributed to: from the
   callee spelling to the last argument, the part of the block that names
   what is checked. *)
let call_span (b : Ast.extern_lang_body) : Span.span =
  match List.rev b.elb_call_args with
  | [] -> b.elb_call_symbol_span
  | last :: _ -> Span.merge b.elb_call_symbol_span (call_arg_span last)

let of_extern ~ext ~owner (d : Ast.extern_decl) : site list =
  List.map
    (fun (b : Ast.extern_lang_body) ->
      {
        ext;
        lang = b.elb_lang;
        kind = (if owner = None then Op else Method);
        owner;
        name = Some d.ed_name;
        span = call_span b;
      })
    d.ed_langs

let of_lang_blocks ~ext ~kind ~name (blocks : Ast.lang_block list) : site list =
  List.map
    (fun (b : Ast.lang_block) ->
      {
        ext;
        lang = b.lb_lang;
        kind;
        owner = None;
        name = Some name;
        span = b.lb_span;
      })
    blocks

let of_ext_lib (ext : string) (body : Ast.ext_lib_body) : site list =
  let paths =
    List.map
      (fun (lp : Ast.lang_path) ->
        {
          ext;
          lang = lp.lp_lang;
          kind = Path;
          owner = None;
          name = None;
          span = lp.lp_path_span;
        })
      body.elib_langs
  in
  let structs =
    List.concat_map
      (fun (s : Ast.foreign_struct) ->
        of_lang_blocks ~ext ~kind:Struct ~name:s.fs_name s.fs_langs)
      body.elib_structs
  in
  let handles =
    List.concat_map
      (fun (t : Ast.opaque_type) ->
        of_lang_blocks ~ext ~kind:Handle ~name:t.opq_name t.opq_langs
        @ List.concat_map
            (of_extern ~ext ~owner:(Some t.opq_name))
            t.opq_methods)
      body.elib_types
  in
  let externs =
    List.concat_map (of_extern ~ext ~owner:None) body.elib_externs
  in
  paths @ structs @ handles @ externs

let of_file (f : Ast.file) : site list =
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.dkind with
      | Ast.DExtLib { body; _ } -> of_ext_lib d.dname body
      | _ -> [])
    f.decls

let to_json (s : site) : Yojson.Safe.t =
  let opt = function None -> `Null | Some v -> `String v in
  `Assoc
    [
      ("ext", `String s.ext);
      ("lang", `String s.lang);
      ("kind", `String (kind_to_string s.kind));
      ("owner", opt s.owner);
      ("name", opt s.name);
      ("span", `String (Span.to_string s.span));
    ]
