open Tono_frontend

(* `.request`: legal only as a direct or ctor-nested argument of an extern
   call that is itself a @header/@query/@body value. TC0087 is the position
   check; the entry-field-construction side of the same reserved name
   (TC0085) is covered by extern_typecheck_test.ml. *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)
let has code src = List.mem code (codes src)

let ext_block =
  {|ext companyauth {
  go { #(github.com/company/auth) }

  op sign(req: http.request): string {
    go { call: #(Sign)(req) }
  }
}
|}

let client header_arg =
  Printf.sprintf
    {|import tono.http
%s
pub struct note { id: string }

pub struct client {
  ep: string @env("EP")

  @http(method: "GET", path: "/notes", endpoint: .ep)
  %s
  op fetch(): note
}
|}
    ext_block header_arg

let legal_direct_arg () =
  let src = client {|@header("Authorization", companyauth.sign(.request))|} in
  Alcotest.(check bool)
    "no TC0087 for a direct call argument" false (has "TC0087" src)

let legal_ctor_nested_arg () =
  let src =
    client
      {|@header("Authorization", companyauth.sign(opts { token: .request }))|}
  in
  Alcotest.(check bool)
    "no TC0087 for a ctor-nested call argument" false (has "TC0087" src)

(* A bare foreign-symbol call nested inside the header's own call argument
   list (no declared extern behind it, unlike "opts { ... }" above) still
   counts as "inside the call the header's value is": [.request] read
   through it is still legal ([Check_request_value.call_arg_diags]'s own
   [CaCall] case). *)
let legal_nested_call_arg () =
  let src =
    client {|@header("Authorization", companyauth.sign("Wrap"(.request)))|}
  in
  Alcotest.(check bool)
    "no TC0087 for a call-nested call argument" false (has "TC0087" src)

let illegal_bare_value () =
  let src = client {|@header("Authorization", .request)|} in
  Alcotest.(check bool) "bare .request rejected" true (has "TC0087" src)

let illegal_in_http () =
  let src =
    Printf.sprintf
      {|import tono.http
%s
pub struct note { id: string }

pub struct client {
  ep: string @env("EP")

  @http(method: "GET", path: "/notes", endpoint: .request)
  op fetch(): note
}
|}
      ext_block
  in
  Alcotest.(check bool) "@http endpoint: rejected" true (has "TC0087" src)

let illegal_in_errors () =
  let src =
    Printf.sprintf
      {|import tono.http
%s
pub struct note { id: string }

@status(404)
pub struct not_found { message: string }

pub struct client {
  ep: string @env("EP")

  @http(method: "GET", path: "/notes", endpoint: .ep)
  @header("Authorization", companyauth.sign(.request))
  @errors(not_found)
  op fetch(): note
}
|}
      ext_block
  in
  Alcotest.(check bool)
    "the legal header call still leaves other traits untouched" false
    (has "TC0087" src)

let legal_kv_wrapped_arg () =
  let src =
    client {|@header(key: "Authorization", value: companyauth.sign(.request))|}
  in
  Alcotest.(check bool)
    "no TC0087 for a key:value-wrapped call argument" false (has "TC0087" src)

let illegal_call_in_query () =
  let src =
    Printf.sprintf
      {|import tono.http
%s
pub struct note { id: string }

pub struct client {
  ep: string @env("EP")

  @http(method: "GET", path: "/notes", endpoint: .ep)
  @query("sig", companyauth.sign(.request))
  op fetch(): note
}
|}
      ext_block
  in
  Alcotest.(check bool)
    "a call reading .request is illegal in @query" true (has "TC0087" src)

(* A call argument referencing a field the entry doesn't declare is
   collected as an ordinary ref by [Entry_scope.op_refs]'s [Ast.ACall] case
   (not silently dropped, and not confused with the reserved `.request`
   exclusion), so it is diagnosed exactly like a bare unresolved reference
   would be. *)
let an_unknown_field_inside_a_call_argument_is_still_diagnosed () =
  let src = client {|@header("Authorization", companyauth.sign(.nope))|} in
  Alcotest.(check bool)
    "some diagnostic fires for the unresolved field" true
    (check src <> [])

let illegal_bare_param () =
  let src = client {|@header("Authorization", companyauth.sign(token))|} in
  Alcotest.(check bool)
    "a bare identifier call argument rejected" true (has "TC0087" src)

let () =
  Alcotest.run "request_value"
    [
      ( "TC0087",
        [
          Alcotest.test_case "legal direct arg" `Quick legal_direct_arg;
          Alcotest.test_case "legal ctor-nested arg" `Quick
            legal_ctor_nested_arg;
          Alcotest.test_case "legal call-nested arg" `Quick
            legal_nested_call_arg;
          Alcotest.test_case "illegal bare value" `Quick illegal_bare_value;
          Alcotest.test_case "illegal in @http" `Quick illegal_in_http;
          Alcotest.test_case "legal header untouched by other traits" `Quick
            illegal_in_errors;
          Alcotest.test_case "illegal bare param in call" `Quick
            illegal_bare_param;
          Alcotest.test_case "legal key:value-wrapped call argument" `Quick
            legal_kv_wrapped_arg;
          Alcotest.test_case "illegal call in @query" `Quick
            illegal_call_in_query;
          Alcotest.test_case "unknown field inside a call argument diagnosed"
            `Quick an_unknown_field_inside_a_call_argument_is_still_diagnosed;
        ] );
    ]
