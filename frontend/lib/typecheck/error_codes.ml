(* Stable typecheck diagnostic codes. Each maps to one well-defined failure so
   tooling and tests can pin behaviour by code rather than message text. *)

(* Every code below is registered through here rather than written as a bare
   string literal, so a copy-paste collision (the same code claimed twice,
   e.g. across a rebase) fails the moment this module loads, not only when a
   test happens to exercise both codes. *)
let seen : (string, unit) Hashtbl.t = Hashtbl.create 128

let register code =
  if Hashtbl.mem seen code then
    invalid_arg
      (Printf.sprintf "error code %s is registered more than once" code)
  else (
    Hashtbl.add seen code ();
    code)

(* Every code registered so far, in no particular order: the uniqueness
   test walks it to prove the table is one-code-one-meaning without knowing
   the constants by name. *)
let registered () = Hashtbl.fold (fun code () acc -> code :: acc) seen []
let unknown_type = register "TC0001"
let duplicate_shape = register "TC0002"

(* TC0003 is intentionally unused: type-parameter scope is resolved during
   lowering, so an out-of-scope name surfaces as [unknown_type] rather than a
   distinct unbound-parameter code. *)
let generic_arity_mismatch = register "TC0004"
let non_generic_applied = register "TC0005"

(* TC0006 (bounds_not_supported) is rejected at parse time; the checker never
   reaches a bound to report, so the code has no constant here. *)
let nullability_conflict = register "TC0007"
let enum_value_duplicate = register "TC0008"
let enum_backing_mismatch = register "TC0009"
let constraint_type_mismatch = register "TC0010"
let constraint_malformed = register "TC0011"
let default_type_mismatch = register "TC0012"
let default_violates_constraint = register "TC0013"
let unresolved_operation_ref = register "TC0014"
let error_status_missing = register "TC0015"
let error_code_invalid = register "TC0016"
let error_discrimination_ambiguous = register "TC0017"
let async_takes_no_arguments = register "TC0018"

(* TC0019, TC0020 are intentionally unused: they covered the per-member HTTP
   binding traits (@httpLabel/@httpPayload), retired in favor of point-of-use
   declaration (@query/@header/@body); see [protocol_trait_invalid] for the
   checks that replaced them. TC0021 stays live: a map or list value still has
   no defined query/header serialization, and now guards the @query/@header
   call-site grammar instead of the retired @httpQuery/@httpHeader member
   traits. *)
let http_map_binding = register "TC0021"

(* TC0022 also survived the member-trait retirement, moved from a nullable
   @httpLabel member to a nullable value interpolated into an @http path: an
   absent value would still collapse the placeholder and leave a hole in the
   URL. *)
let http_path_nullable_ref = register "TC0022"

(* Module system: a qualified reference names a module not brought into scope by
   an import; a reference resolves to a shape that is not [pub] in its module; the
   module import graph contains a cycle (it must be a DAG); two imports in one
   module resolve to the same qualifier, so one would silently shadow the other. *)
let unknown_import = register "TC0023"
let not_exported = register "TC0024"
let module_cycle = register "TC0025"
let duplicate_import = register "TC0026"
let ext_hook_removed = register "TC0027"
let ext_binding_language_invalid = register "TC0028"
let ext_signature_rule = register "TC0029"
let ext_binding_missing = register "TC0030"
let ext_binding_malformed = register "TC0031"
let ext_duplicate = register "TC0032"
let ext_binding_duplicate_language = register "TC0033"

(* Entry model: roles emerge from struct content and their boundaries are
   closed. An entry/config crossing the wire boundary (op input/output/error, or
   a wire member's type); a source trait in a position that cannot carry one; a
   source that can never fire (@arg combined with anything else, match combined
   with sources); a consumed field whose resolution chain reaches a field with
   no declared source; an unresolvable field reference; a resolution cycle;
   a malformed match (subject/pattern/arm); a non-exhaustive match; a @bind
   outside its composition point; an entry @http op without an endpoint ref; a
   protocol trait on an op that cannot carry it; an unknown transform catalog
   entry; an entry/config shape rule (generics, nullable field). *)
let entry_wire_boundary = register "TC0034"
let source_position_invalid = register "TC0035"
let source_dead = register "TC0036"
let field_unresolvable = register "TC0037"
let field_ref_unknown = register "TC0038"
let resolution_cycle = register "TC0039"
let match_invalid = register "TC0040"
let match_not_exhaustive = register "TC0041"
let bind_invalid = register "TC0042"
let entry_endpoint_missing = register "TC0043"
let protocol_trait_invalid = register "TC0044"
let transform_unknown = register "TC0045"
let entry_shape_invalid = register "TC0046"

(* A second occurrence of a non-repeatable trait (@doc, @http, ...) on one
   declaration or member. *)
let duplicate_trait = register "TC0047"

(* Bespoke operation implementations. An "ext impl" names the operation it
   implements, so the name must reach exactly one operation an entry declares:
   none is an orphan, a loose one gets no generated body, and several is an
   ambiguous bare name that "entry.op" resolves. An
   entry operation is implemented exactly once, by a protocol binding or by an
   impl but never by both and never by neither. The raw response form belongs to
   an impl: on any other kind it would silently do nothing. *)
let ext_impl_unknown_op = register "TC0048"
let ext_impl_ambiguous_op = register "TC0049"
let op_implementation_conflict = register "TC0050"
let op_implementation_missing = register "TC0051"
let ext_raw_rule = register "TC0052"

(* A raw implementation reports a failure by its code alone: it carries no
   protocol status to match on. A declared error with no @errorCode therefore has
   nothing that could select it, so the generated glue would resolve it to the
   generic fallback instead. Reported as a warning, not an error: the operation
   still works, and adding the code is the fix. *)
let raw_error_unreachable = register "TC0053"

(* A trait the compiler does not read. Bare trait names are matched by the
   checkers that act on them and ignored everywhere else, so a misspelling used
   to reach the IR and the generated SDK doing nothing. Reported as a warning,
   not an error: the IR's trait vocabulary is open, so an unread trait is inert
   rather than malformed. *)
let unknown_trait = register "TC0054"

(* Declared tests. Every name in a test resolves backwards to a binding the
   test declared; the stub, call, value, and pattern rules below are the
   closed grammar of a declared test. The tono.* language modules (tono.http,
   tono.errors) provide the http/errors shapes and must be imported before use;
   the tono.* module root is reserved for the language. *)
let test_binding_unknown = register "TC0055"
let test_op_unknown = register "TC0056"
let test_dep_invalid = register "TC0057"
let test_stub_value_invalid = register "TC0058"
let test_value_invalid = register "TC0059"
let test_pattern_invalid = register "TC0060"
let test_expect_missing = register "TC0061"
let test_requests_subject_invalid = register "TC0062"
let test_shape_ambiguous = register "TC0063"
let test_import_missing = register "TC0064"
let tono_root_reserved = register "TC0065"
let test_binding_duplicate = register "TC0066"

(* Two declared errors of one operation share a status but read their
   @errorCode from different paths. Legal (each guard probes its own
   location), but unusual enough to flag. *)
let error_code_paths_diverge = register "TC0067"

(* @http(code:) is an int or a non-empty list of ints; anything else (an empty
   list, a non-int element, a non-int scalar) would otherwise fall silently
   into the "no code declared" default instead of the exact match the author
   wrote. *)
let http_code_invalid = register "TC0068"

(* A known trait written where nothing reads it: a member-scoped trait
   (@range, @required, @arg, @format, @httpResponseCode, ...) on a shape, op,
   union variant, or enum case, or an op-scoped trait (@http, @header,
   @retry, ...) anywhere but an op. The name is real, so [unknown_trait]
   stays silent; the value is still dropped with nothing to say so. The
   op traits and the error taxonomy grouped in [Trait_vocab.operations]
   (@async, @errorCode, @status, ...) are out of scope: @errorCode/@status are
   legal on a declared error shape as well as an op, and the group has no
   single position to check the rest of its members against, so this rule
   does not cover it. *)
let trait_position_invalid = register "TC0069"

(* The "ext"/"extern" FFI library block. A call: arg names a
   parameter the extern's logical signature never declared; a ctor literal
   projected into a foreign struct disagrees in field name or type with the
   parameter it forwards. A yields: position nothing in returns:/errors:
   reads; more than one "error"-typed position in one yields:; a returns:
   with no yields: to project from; a returns: that builds a type other than
   the extern's own declared logical return; a returns: field ref whose head
   is not a declared yields: name. An errors: sentinel mapped to a type that
   does not resolve. A logical parameter some language's call: never
   consumes. Cross-file closed accounting (decision K): the same "ext" name's
   module path for one language declared with two different targets; an
   extern (or opaque-type method) name repeated within one "ext", even across
   files; a language block for a target the "ext" declares no module path
   for. *)
let extern_call_unknown_param = register "TC0070"
let extern_call_type_mismatch = register "TC0071"
let extern_yields_position_dead = register "TC0072"
let extern_yields_multiple_errors = register "TC0073"
let extern_yields_required = register "TC0074"
let extern_returns_type_mismatch = register "TC0075"
let extern_returns_ref_unknown = register "TC0076"
let extern_error_unknown = register "TC0077"
let extern_param_unconsumed = register "TC0078"
let ext_lib_module_path_conflict = register "TC0079"
let extern_duplicate_name = register "TC0080"
let extern_lang_no_module = register "TC0081"

(* A call into a declared opaque handle's method (an op's own "impl
   .field.method(args)" body, or a field's own "= .field.method(args)"
   source). The receiver does not resolve to an entry field whose type is a
   declared opaque handle; the method is not one of that handle's declared
   "extern" methods; the argument list disagrees in count with the method's
   declared logical parameters, or an argument is a bare identifier (no
   extern-side parameter list exists to forward from in this position; only
   a literal or a field reference is legal). *)
let op_impl_receiver_not_handle = register "TC0082"
let op_impl_unknown_method = register "TC0083"
let op_impl_arity_mismatch = register "TC0084"

(* An entry field's `= ns.fn(args)` call reads `.request` in its own
   argument list. The canonical request only exists once a protocol trait
   argument's call reads it; during field construction it has not been
   built yet, so this gets its own message rather than reading as an
   ordinary unknown-field reference. *)
let field_ref_request = register "TC0085"

(* An op's named parameter has the same name as an entry field, so a `.name`
   ref in that op now resolves to the parameter instead of the field.
   Previously TC0048, which collided with [ext_impl_unknown_op]; codes are
   the stable identifier tooling keys on, so this moved rather than the
   sequential TC0048-TC0053 bespoke-implementation block above. *)
let param_shadows_field = register "TC0086"

(* `.request` referenced outside the one legal position: a direct or
   ctor-nested argument to an extern call that is itself the value of a
   @header/@query/@body trait. Anywhere else it is reserved and
   unresolvable — a bare use, a use inside another trait (@http, @timeout,
   @retry, @errors, ...), or a bare identifier passed to the call in its
   place. The entry-field-construction side of the same reserved name is
   [field_ref_request]. *)
let request_value_invalid = register "TC0087"

(* A map-index key (the "[.seg]" in "map[.seg]") does not resolve, or its
   type does not match the map's declared key type. *)
let map_index_key_invalid = register "TC0088"

(* A match subject typed T? (optional, from a map index or an otherwise
   nullable field) must include exactly one "null" arm; it is missing here. *)
let match_missing_null_arm = register "TC0089"

(* A "null" pattern arm on a match whose subject is not optional: "null" is
   only meaningful when the subject can be absent. *)
let match_null_arm_not_optional = register "TC0090"

(* "._" (the match subject shorthand) used somewhere other than a match
   arm's value position, or inside the "null" arm's value (where the subject
   is by definition absent, so there is nothing for "._" to name). *)
let match_subject_ref_invalid = register "TC0091"

(* A language block on a top-level struct that is an entry or a config: a
   block says how a target recognizes a foreign error (an error struct) or
   declares a target's field tags (a wire struct), and neither has a use on
   a struct that is constructed rather than read. *)
let struct_lang_block_misplaced = register "TC0092"

(* @async on an ext op naming a target that has no asynchronous call (Go
   has no await), or one the ext declares no module path for: the trait
   lists where the foreign call is asynchronous, so a target it cannot
   apply to is an error, not a no-op. *)
let extern_async_target_invalid = register "TC0093"

(* A field's own "= .field.method(args)" source names a method whose
   declared logical return is not the field's declared type: the value is
   stored as-is, so the two must agree. *)
let handle_call_type_mismatch = register "TC0094"

(* A language block that does not fit its struct: the same language twice
   on one struct, a language the enclosing ext declares no module path for
   (same spirit as TC0081), or, on a top-level error struct, a language that
   is not a target at all. *)
let lang_block_mismatch = register "TC0095"

(* A trait on an ext op that is not one the boundary accepts (@async,
   @errors, @doc): the op's behaviour is declared by its language blocks,
   and a trait from the rest of the language has no meaning there. *)
let extern_trait_invalid = register "TC0096"

(* A keyed entry of a language block naming no field of its struct, or a
   keyed entry on an opaque handle at all (a handle has no fields; its block
   is only the storage type). *)
let lang_block_field_unknown = register "TC0097"

(* A bare name in a call: line that is both a logical parameter of the op
   and a class reference (an opaque handle of the same ext block, or a
   struct of the module): the parameter would be read where the class
   reference was meant (or the other way round), so the collision is named
   instead of resolved one way. *)
let extern_name_ambiguous = register "TC0098"

(* A returns: whose type is an opaque handle of the same ext block. A handle
   is what the call itself returns, never a projection: there are no fields
   to build it from, so the binding declares the call's positions with
   yields: alone (and the target compiler grades the value). *)
let extern_returns_handle = register "TC0099"

(* A language block whose shape does not fit its struct: a head where none
   belongs (a wire struct's block declares field tags only), no head where
   one is required (a foreign form, an opaque handle, an error struct each
   name something foreign first), or a wire struct's block with no entry
   at all. *)
let lang_block_shape = register "TC0100"
