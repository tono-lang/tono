open Tono_frontend

(* The typecheck diagnostic codes are a stable public surface: every code
   is registered exactly once, spelled TCnnnn, and a second registration of
   a taken code is refused the moment it happens. *)

let every_code_is_unique_and_well_formed () =
  let codes = List.sort compare (Error_codes.registered ()) in
  Alcotest.(check bool)
    "at least the first code exists" true (List.mem "TC0001" codes);
  List.iter
    (fun code ->
      Alcotest.(check bool)
        (code ^ " is TCnnnn") true
        (String.length code = 6
        && String.sub code 0 2 = "TC"
        && String.for_all (fun c -> c >= '0' && c <= '9') (String.sub code 2 4)
        ))
    codes;
  let rec no_dup = function
    | a :: (b :: _ as rest) -> a <> b && no_dup rest
    | _ -> true
  in
  Alcotest.(check bool) "no code is registered twice" true (no_dup codes)

let a_taken_code_is_refused () =
  match Error_codes.register "TC0001" with
  | _ -> Alcotest.fail "a second registration must raise"
  | exception Invalid_argument msg ->
      Alcotest.(check bool)
        "names the code" true
        (Option.is_some
           (Str.search_forward (Str.regexp_string "TC0001") msg 0 |> Option.some))

let () =
  Alcotest.run "error_codes"
    [
      ( "codes",
        [
          Alcotest.test_case "unique and well-formed" `Quick
            every_code_is_unique_and_well_formed;
          Alcotest.test_case "taken code refused" `Quick a_taken_code_is_refused;
        ] );
    ]
