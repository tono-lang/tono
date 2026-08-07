(* The traits a source file may write, in one place.

   The vocabulary was scattered: every checker pattern-matches the keys it cares
   about and ignores the rest, which is what let a misspelled trait through in
   silence. It reached the trait bag, the IR, and the generated SDK, doing
   nothing anywhere and saying so nowhere.

   Collecting the names here does not close the model. The IR's trait vocabulary
   is open by design (a trait is an id and an arbitrary JSON value, carried
   through unchanged), and namespaced traits (@str:: and @num:: catalogs) are
   validated elsewhere. This list covers only the bare names the
   compiler itself acts on, so a name outside it can be reported as one nothing
   will read. *)

(* Grouped by what reads them, so a new trait is added next to its kin and the
   set stays reviewable against the checkers it mirrors. *)

(* Constraints lifted onto a member or entry field (Lower). *)
let constraints = [ "range"; "length"; "pattern"; "multipleOf" ]

(* Member shape: presence and the value a missing one takes (Lower, Check_member). *)
let members = [ "required"; "default" ]

(* Where an entry field's value comes from, and what is done to it
   (Entry_scope.source_names, Lower). *)
let sources = [ "arg"; "with"; "env" ]
let entry_fields = [ "format"; "bind"; "entries" ]

(* The HTTP protocol binding: the operation's endpoint, and how each output
   member is read from the response (Protocol_http). The request side is
   declared entirely at the point of use now (Entry_scope.protocol_trait_names). *)
let http = [ "http"; "httpResponseCode" ]

(* Per-request protocol knobs an entry supplies (Entry_scope.protocol_trait_names). *)
let protocol = [ "header"; "query"; "body"; "timeout"; "retry" ]

(* Operations and the error taxonomy they raise (Check_operations, Ops). *)
let operations = [ "async"; "errors"; "errorCode"; "status"; "retryable" ]

(* Shape-level structure and the surface the generated SDK presents
   (Lower, the backend's trait reader). *)
let surface = [ "doc"; "deprecated"; "rename"; "wire"; "discriminator" ]

let known =
  constraints @ members @ sources @ entry_fields @ http @ protocol @ operations
  @ surface

let is_known (name : string) : bool = List.mem name known

(* A namespaced trait (@str::trim) belongs to a catalog with its own validation,
   so it is not this list's to judge. *)
let is_namespaced (name : string) : bool = String.contains name ':'

let levenshtein (a : string) (b : string) : int =
  let la = String.length a and lb = String.length b in
  let prev = Array.init (lb + 1) Fun.id in
  let cur = Array.make (lb + 1) 0 in
  for i = 1 to la do
    cur.(0) <- i;
    for j = 1 to lb do
      let cost = if a.[i - 1] = b.[j - 1] then 0 else 1 in
      cur.(j) <- min (min (cur.(j - 1) + 1) (prev.(j - 1) + cost)) (prev.(j) + 1)
    done;
    Array.blit cur 0 prev 0 (lb + 1)
  done;
  prev.(lb)

(* The nearest known trait, when one is near enough to be worth naming. The
   bound scales with the word: a third of its length (at least one edit) keeps
   "@statusx" pointing at @status without pairing every short unknown name with an
   unrelated short known one. *)
let nearest (name : string) : string option =
  let budget = max 1 (String.length name / 3) in
  let best =
    List.fold_left
      (fun acc candidate ->
        let d = levenshtein name candidate in
        match acc with
        | Some (_, bd) when bd <= d -> acc
        | _ -> Some (candidate, d))
      None known
  in
  match best with
  | Some (candidate, d) when d <= budget -> Some candidate
  | _ -> None
