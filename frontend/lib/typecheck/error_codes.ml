(* Stable typecheck diagnostic codes. Each maps to one well-defined failure so
   tooling and tests can pin behaviour by code rather than message text. *)

let unknown_type = "TC0001"
let duplicate_shape = "TC0002"

(* TC0003 is intentionally unused: type-parameter scope is resolved during
   lowering, so an out-of-scope name surfaces as [unknown_type] rather than a
   distinct unbound-parameter code. *)
let generic_arity_mismatch = "TC0004"
let non_generic_applied = "TC0005"

(* TC0006 (bounds_not_supported) is rejected at parse time; the checker never
   reaches a bound to report, so the code has no constant here. *)
let nullability_conflict = "TC0007"
let enum_value_duplicate = "TC0008"
let enum_backing_mismatch = "TC0009"
let constraint_type_mismatch = "TC0010"
let constraint_malformed = "TC0011"
let default_type_mismatch = "TC0012"
let default_violates_constraint = "TC0013"
let unresolved_operation_ref = "TC0014"
let error_status_missing = "TC0015"
let error_code_invalid = "TC0016"
let error_discrimination_ambiguous = "TC0017"
let async_takes_no_arguments = "TC0018"

(* TC0019, TC0020 are intentionally unused: they covered the per-member HTTP
   binding traits (@httpLabel/@httpPayload), retired in favor of point-of-use
   declaration (@query/@header/@body); see [protocol_trait_invalid] for the
   checks that replaced them. TC0021 stays live: a map or list value still has
   no defined query/header serialization, and now guards the @query/@header
   call-site grammar instead of the retired @httpQuery/@httpHeader member
   traits. *)
let http_map_binding = "TC0021"

(* TC0022 also survived the member-trait retirement, moved from a nullable
   @httpLabel member to a nullable value interpolated into an @http path: an
   absent value would still collapse the placeholder and leave a hole in the
   URL. *)
let http_path_nullable_ref = "TC0022"

(* Module system: a qualified reference names a module not brought into scope by
   an import; a reference resolves to a shape that is not [pub] in its module; the
   module import graph contains a cycle (it must be a DAG); two imports in one
   module resolve to the same qualifier, so one would silently shadow the other. *)
let unknown_import = "TC0023"
let not_exported = "TC0024"
let module_cycle = "TC0025"
let duplicate_import = "TC0026"
let ext_unknown_hook_slot = "TC0027"
let ext_binding_language_invalid = "TC0028"
let ext_signature_rule = "TC0029"
let ext_binding_missing = "TC0030"
let ext_binding_malformed = "TC0031"
let ext_duplicate = "TC0032"
let ext_binding_duplicate_language = "TC0033"

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
let entry_wire_boundary = "TC0034"
let source_position_invalid = "TC0035"
let source_dead = "TC0036"
let field_unresolvable = "TC0037"
let field_ref_unknown = "TC0038"
let resolution_cycle = "TC0039"
let match_invalid = "TC0040"
let match_not_exhaustive = "TC0041"
let bind_invalid = "TC0042"
let entry_endpoint_missing = "TC0043"
let protocol_trait_invalid = "TC0044"
let transform_unknown = "TC0045"
let entry_shape_invalid = "TC0046"

(* A second occurrence of a non-repeatable trait (@doc, @http, ...) on one
   declaration or member; usually the trailing-trait absorption footgun. *)
let duplicate_trait = "TC0047"

(* Bespoke operation implementations. An "ext impl" names the operation it
   implements, so the name must reach exactly one operation an entry declares:
   none is an orphan, a loose one gets no generated body, and several is an
   ambiguous bare name that "entry.op" resolves. An
   entry operation is implemented exactly once, by a protocol binding or by an
   impl but never by both and never by neither. The raw response form belongs to
   an impl: on any other kind it would silently do nothing. *)
let ext_impl_unknown_op = "TC0048"
let ext_impl_ambiguous_op = "TC0049"
let op_implementation_conflict = "TC0050"
let op_implementation_missing = "TC0051"
let ext_raw_rule = "TC0052"

(* A raw implementation reports a failure by its code alone: it carries no
   protocol status to match on. A declared error with no @errorCode therefore has
   nothing that could select it, so the generated glue would resolve it to the
   generic fallback instead. Reported as a warning, not an error: the operation
   still works, and adding the code is the fix. *)
let raw_error_unreachable = "TC0053"

(* A trait the compiler does not read. Bare trait names are matched by the
   checkers that act on them and ignored everywhere else, so a misspelling used
   to reach the IR and the generated SDK doing nothing. Reported as a warning,
   not an error: the IR's trait vocabulary is open, so an unread trait is inert
   rather than malformed. *)
let unknown_trait = "TC0054"

(* Declared tests. Every name in a test resolves backwards to a binding the
   test declared; the stub, call, value, and pattern rules below are the closed
   grammar of RFC-declared tests. The tono.* language modules (tono.http,
   tono.errors) provide the http/errors shapes and must be imported before use;
   the tono.* module root is reserved for the language. *)
let test_binding_unknown = "TC0055"
let test_op_unknown = "TC0056"
let test_dep_invalid = "TC0057"
let test_stub_value_invalid = "TC0058"
let test_value_invalid = "TC0059"
let test_pattern_invalid = "TC0060"
let test_expect_missing = "TC0061"
let test_requests_subject_invalid = "TC0062"
let test_shape_ambiguous = "TC0063"
let test_import_missing = "TC0064"
let tono_root_reserved = "TC0065"
let test_binding_duplicate = "TC0066"

(* Two declared errors of one operation share a status but read their
   @errorCode from different paths. Legal (each guard probes its own
   location), but unusual enough to flag. *)
let error_code_paths_diverge = "TC0067"

(* @http(code:) is an int or a non-empty list of ints; anything else (an empty
   list, a non-int element, a non-int scalar) would otherwise fall silently
   into the "no code declared" default instead of the exact match the author
   wrote. *)
let http_code_invalid = "TC0068"

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
let trait_position_invalid = "TC0069"

(* The "ext"/"extern" FFI library block (RFC-0023). A call: arg names a
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
let extern_call_unknown_param = "TC0070"
let extern_call_type_mismatch = "TC0071"
let extern_yields_position_dead = "TC0072"
let extern_yields_multiple_errors = "TC0073"
let extern_yields_required = "TC0074"
let extern_returns_type_mismatch = "TC0075"
let extern_returns_ref_unknown = "TC0076"
let extern_error_sentinel_unknown = "TC0077"
let extern_param_unconsumed = "TC0078"
let ext_lib_module_path_conflict = "TC0079"
let extern_duplicate_name = "TC0080"
let extern_lang_no_module = "TC0081"

(* An op's own "impl .field.method(args)" body (RFC-0023). The receiver does
   not resolve to an entry field whose type is a declared opaque handle; the
   method is not one of that handle's declared "extern" methods; the
   argument list disagrees in count with the method's declared logical
   parameters, or an argument is a bare identifier (no extern-side
   parameter list exists to forward from in this position; only a literal
   or a field reference is legal). *)
let op_impl_receiver_not_handle = "TC0082"
let op_impl_unknown_method = "TC0083"
let op_impl_arity_mismatch = "TC0084"

(* An entry field's `= ns.fn(args)` call reads `.request` in its own
   argument list. The canonical request only exists once a protocol trait
   argument's call reads it; during field construction it has not been
   built yet, so this gets its own message rather than reading as an
   ordinary unknown-field reference. *)
let field_ref_request = "TC0085"

(* An op's named parameter has the same name as an entry field, so a `.name`
   ref in that op now resolves to the parameter instead of the field.
   Previously TC0048, which collided with [ext_impl_unknown_op]; codes are
   the stable identifier tooling keys on, so this moved rather than the
   sequential TC0048-TC0053 bespoke-implementation block above. *)
let param_shadows_field = "TC0086"

(* `.request` referenced outside the one legal position: a direct or
   ctor-nested argument to an extern call that is itself the value of a
   @header/@query/@body trait. Anywhere else it is reserved and
   unresolvable — a bare use, a use inside another trait (@http, @timeout,
   @retry, @errors, ...), or a bare identifier passed to the call in its
   place. The entry-field-construction side of the same reserved name is
   [field_ref_request]. *)
let request_value_invalid = "TC0087"
