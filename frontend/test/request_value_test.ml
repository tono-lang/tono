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
  go: "github.com/company/auth"

  extern sign(req: http.request): string {
    go { call: "Sign"(req) }
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

  op fetch(): note
    @http(method: "GET", path: "/notes", endpoint: .ep)
    %s
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

  op fetch(): note
    @http(method: "GET", path: "/notes", endpoint: .request)
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

  op fetch(): note
    @http(method: "GET", path: "/notes", endpoint: .ep)
    @header("Authorization", companyauth.sign(.request))
    @errors(not_found)
}
|}
      ext_block
  in
  Alcotest.(check bool)
    "the legal header call still leaves other traits untouched" false
    (has "TC0087" src)

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
          Alcotest.test_case "illegal bare value" `Quick illegal_bare_value;
          Alcotest.test_case "illegal in @http" `Quick illegal_in_http;
          Alcotest.test_case "legal header untouched by other traits" `Quick
            illegal_in_errors;
          Alcotest.test_case "illegal bare param in call" `Quick
            illegal_bare_param;
        ] );
    ]
