open Tono_frontend

(* Extension-model checks (TC0027-TC0033): the closed hook rejection and its
   substitute messages, the contract/language/binding structural rules. Split
   out of typecheck_test.ml to stay under the repo's file-size gate. *)

(* Parse + lower a snippet, then run the typecheck pass directly, returning its
   diagnostics in isolation from lowering's own (which the helper discards). *)
let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

let contains (needle : string) (haystack : string) : bool =
  let n = String.length needle and h = String.length haystack in
  let rec go i =
    i + n <= h && (String.sub haystack i n = needle || go (i + 1))
  in
  n = 0 || go 0

(* A contract with a signature and conformance, and a constraint, both pass the
   structural checks. *)
let ext_clean () =
  Alcotest.(check (list string))
    "no codes" []
    (codes
       "ext contract sign (string) -> string { rust: \"ext/rust/s.rs#g\", \
        conformance: \"v.json\" }\n\
        ext constraint luhn (string) -> bool { go: \"ext/go/l.go#H\" }")

(* Every hook is rejected outright, regardless of whether its name matches one
   of the four former lifecycle slots: the lifecycle itself is gone. A known
   slot name gets its specific substitute; an unrecognized name gets the
   generic pointer to 'ext <lib> { extern ... }'. *)
let ext_hook_client_init_removed () =
  Alcotest.(check (list string))
    "client_init hook removed" [ "TC0027" ]
    (codes "ext hook client_init { ts: \"ext/ts/a.ts#f\" }")

let ext_hook_before_request_removed () =
  Alcotest.(check (list string))
    "before_request hook removed" [ "TC0027" ]
    (codes "ext hook before_request { ts: \"ext/ts/a.ts#f\" }")

let ext_hook_after_response_removed () =
  Alcotest.(check (list string))
    "after_response hook removed" [ "TC0027" ]
    (codes "ext hook after_response { ts: \"ext/ts/a.ts#f\" }")

let ext_hook_on_error_removed () =
  Alcotest.(check (list string))
    "on_error hook removed" [ "TC0027" ]
    (codes "ext hook on_error { ts: \"ext/ts/a.ts#f\" }")

let ext_hook_unknown_slot_removed () =
  Alcotest.(check (list string))
    "unknown hook name still rejected" [ "TC0027" ]
    (codes "ext hook on_startup { ts: \"ext/ts/a.ts#f\" }")

let ext_hook_with_signature_still_removed () =
  (* A hook is rejected before the signature check can even run: there is no
     "hook declares no signature" diagnostic left to raise, only TC0027. *)
  Alcotest.(check (list string))
    "hook removed regardless of signature" [ "TC0027" ]
    (codes
       "ext hook before_request (string) -> string { ts: \"ext/ts/a.ts#f\" }")

let ext_contract_without_signature () =
  Alcotest.(check (list string))
    "contract needs a signature" [ "TC0029" ]
    (codes "ext contract sign { ts: \"ext/ts/s.ts#g\" }")

let ext_bad_language () =
  Alcotest.(check (list string))
    "unsupported binding language" [ "TC0028" ]
    (codes "ext constraint luhn (string) -> bool { cobol: \"ext/cobol/a#f\" }")

let ext_no_binding () =
  Alcotest.(check (list string))
    "extension with no binding" [ "TC0030" ]
    (codes "ext constraint luhn (string) -> bool {}")

let ext_malformed_binding () =
  Alcotest.(check (list string))
    "binding is not file#symbol" [ "TC0031" ]
    (codes "ext constraint luhn (string) -> bool { ts: \"ext/ts/luhn.ts\" }")

let ext_duplicate_slot () =
  Alcotest.(check (list string))
    "duplicate extension name" [ "TC0032" ]
    (codes
       "ext constraint luhn (string) -> bool { ts: \"ext/ts/a.ts#f\" }\n\
        ext constraint luhn (string) -> bool { rust: \"ext/rust/b.rs#g\" }")

let ext_duplicate_language () =
  Alcotest.(check (list string))
    "language bound twice" [ "TC0033" ]
    (codes
       "ext constraint luhn (string) -> bool { ts: \"ext/ts/a.ts#f\" ts: \
        \"ext/ts/b.ts#g\" }")

(* Each of the four removed lifecycle slots names its own substitute (the ext/extern
   FFI model replaced the hooks); an unrecognized hook name gets the generic pointer to
   the ext/extern FFI model instead. *)
let ext_hook_message_names_substitute () =
  let hint_of src =
    List.map (fun (d : Diagnostic.t) -> d.message) (check src)
  in
  Alcotest.(check bool)
    "client_init points at extern field or @format+@header" true
    (List.exists (contains "extern")
       (hint_of "ext hook client_init { ts: \"ext/ts/a.ts#f\" }"));
  Alcotest.(check bool)
    "before_request points at a trait-argument value" true
    (List.exists
       (contains "bind an external value to a trait argument")
       (hint_of "ext hook before_request { ts: \"ext/ts/a.ts#f\" }"));
  Alcotest.(check bool)
    "after_response points at the op's returns: projection" true
    (List.exists
       (contains "extern returns:")
       (hint_of "ext hook after_response { ts: \"ext/ts/a.ts#f\" }"));
  Alcotest.(check bool)
    "on_error points at errors: plus @errors" true
    (List.exists (contains "errors:")
       (hint_of "ext hook on_error { ts: \"ext/ts/a.ts#f\" }"));
  Alcotest.(check bool)
    "unknown hook name gets the generic pointer" true
    (List.exists
       (contains "ext <lib> { extern")
       (hint_of "ext hook on_startup { ts: \"ext/ts/a.ts#f\" }"))

let () =
  Alcotest.run "typecheck-ext"
    [
      ( "extensions",
        [
          Alcotest.test_case "clean" `Quick ext_clean;
          Alcotest.test_case "client_init hook removed" `Quick
            ext_hook_client_init_removed;
          Alcotest.test_case "before_request hook removed" `Quick
            ext_hook_before_request_removed;
          Alcotest.test_case "after_response hook removed" `Quick
            ext_hook_after_response_removed;
          Alcotest.test_case "on_error hook removed" `Quick
            ext_hook_on_error_removed;
          Alcotest.test_case "unknown hook name removed" `Quick
            ext_hook_unknown_slot_removed;
          Alcotest.test_case "hook with signature still removed" `Quick
            ext_hook_with_signature_still_removed;
          Alcotest.test_case "hook message names substitute" `Quick
            ext_hook_message_names_substitute;
          Alcotest.test_case "contract without signature" `Quick
            ext_contract_without_signature;
          Alcotest.test_case "bad language" `Quick ext_bad_language;
          Alcotest.test_case "no binding" `Quick ext_no_binding;
          Alcotest.test_case "malformed binding" `Quick ext_malformed_binding;
          Alcotest.test_case "duplicate slot" `Quick ext_duplicate_slot;
          Alcotest.test_case "duplicate language" `Quick ext_duplicate_language;
        ] );
    ]
