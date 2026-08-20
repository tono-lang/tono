(* Stable typecheck diagnostic codes. Each maps to one well-defined failure so
   tooling and tests can pin behaviour by code rather than message text. *)

val unknown_type : string
val duplicate_shape : string

(* TC0003 is intentionally unused: type-parameter scope is resolved during
   lowering, so an out-of-scope name surfaces as [unknown_type]. *)
val generic_arity_mismatch : string
val non_generic_applied : string

(* TC0006 (bounds_not_supported) is a parse-level rejection; no constant here. *)
val nullability_conflict : string
val enum_value_duplicate : string
val enum_backing_mismatch : string
val constraint_type_mismatch : string
val constraint_malformed : string
val default_type_mismatch : string
val default_violates_constraint : string
val unresolved_operation_ref : string
val error_status_missing : string
val error_code_invalid : string
val error_discrimination_ambiguous : string
val async_takes_no_arguments : string

(* TC0019, TC0020 are intentionally unused (see error_codes.ml). *)
val http_map_binding : string

(* A nullable value interpolated into an @http path (see error_codes.ml). *)
val http_path_nullable_ref : string

(* Module system: unknown import qualifier, non-[pub] reference, a cycle in the
   module import graph (which must be a DAG), and two imports colliding on one
   qualifier. *)
val unknown_import : string
val not_exported : string
val module_cycle : string
val duplicate_import : string

(* Extension-model checks: a hook (removed, see error_codes.ml), an unsupported
   binding language, a kind/signature-shape violation, an extension with no
   binding, a binding that is not a "file#symbol" reference, a duplicate
   extension name, and a language bound twice within one extension (which
   would collapse to one wire key). *)
val ext_hook_removed : string
val ext_binding_language_invalid : string
val ext_signature_rule : string
val ext_binding_missing : string
val ext_binding_malformed : string
val ext_duplicate : string
val ext_binding_duplicate_language : string

(* Entry model: closed boundaries between entry/config and wire, declared value
   sources, table selection, composition, and protocol trait positions. *)
val entry_wire_boundary : string
val source_position_invalid : string
val source_dead : string
val field_unresolvable : string
val field_ref_unknown : string
val resolution_cycle : string
val match_invalid : string
val match_not_exhaustive : string
val bind_invalid : string
val entry_endpoint_missing : string
val protocol_trait_invalid : string
val transform_unknown : string
val entry_shape_invalid : string
val duplicate_trait : string

(* Bespoke operation implementations: an impl naming no implementable operation
   or an ambiguous one, an operation implemented twice or not at all, and the raw
   response form on a kind that cannot carry it. *)
val ext_impl_unknown_op : string
val ext_impl_ambiguous_op : string
val op_implementation_conflict : string
val op_implementation_missing : string
val ext_raw_rule : string

(* A declared error a raw implementation could never select: the raw path
   discriminates by @errorCode alone, so an error without one is unreachable. *)
val raw_error_unreachable : string

(* A bare trait outside the compiler's vocabulary: carried into the IR but read
   by nothing, so almost always a misspelling. *)
val unknown_trait : string

(* Declared tests (see error_codes.ml for the rule commentary). *)
val test_binding_unknown : string
val test_op_unknown : string
val test_dep_invalid : string
val test_stub_value_invalid : string
val test_value_invalid : string
val test_pattern_invalid : string
val test_expect_missing : string
val test_requests_subject_invalid : string
val test_shape_ambiguous : string
val test_import_missing : string
val tono_root_reserved : string
val test_binding_duplicate : string

(* Two declared errors of one operation share a status but read their
   @errorCode from different paths (see error_codes.ml for the rule). *)
val error_code_paths_diverge : string
val http_code_invalid : string

(* A known trait written where nothing reads it (see error_codes.ml for the
   rule). *)
val trait_position_invalid : string

(* The "ext"/"extern" FFI library block (see error_codes.ml for the
   rule commentary). Internal consistency of a call's arity/types against the
   declared logical signature, of a returns: projection against a yields:
   name, and of an errors: sentinel against a declared error; plus the
   cross-file closed accounting of one library split across several .tono
   files. Foreign-role boundary violations (a foreign form used as wire, an
   op input/output, or public surface) reuse [entry_wire_boundary]. *)
val extern_call_unknown_param : string
val extern_call_type_mismatch : string
val extern_yields_position_dead : string
val extern_yields_multiple_errors : string
val extern_yields_required : string
val extern_returns_type_mismatch : string
val extern_returns_ref_unknown : string
val extern_error_sentinel_unknown : string
val extern_param_unconsumed : string
val ext_lib_module_path_conflict : string
val extern_duplicate_name : string
val extern_lang_no_module : string

(* A call into a declared opaque handle's method (an op's "impl" body or a
   field's "= .field.method(args)" source): the receiver is not an entry
   field whose type is a declared opaque handle; the method is not one of
   that handle's declared "extern" methods; the argument count disagrees
   with the method's declared logical parameters, or an argument is a bare
   identifier (only a literal or a field reference is legal in this
   position, since there is no extern-side parameter list to forward
   from). *)
val op_impl_receiver_not_handle : string
val op_impl_unknown_method : string
val op_impl_arity_mismatch : string

(* `.request` referenced from an entry field's extern-call arguments (see
   error_codes.ml for the rule). *)
val field_ref_request : string

(* An op's named parameter shadowing an entry field of the same name (see
   error_codes.ml for the rule; moved here from the former TC0048, which
   collided with [ext_impl_unknown_op]). *)
val param_shadows_field : string

(* `.request` used outside a @header/@query/@body extern-call argument (see
   error_codes.ml for the rule). *)
val request_value_invalid : string

(* Map-index key resolution/type mismatch, missing/misplaced mandatory
   "null" match arm, and misplaced "._" (see error_codes.ml for the rules). *)
val map_index_key_invalid : string
val match_missing_null_arm : string
val match_null_arm_not_optional : string
val match_subject_ref_invalid : string

(* An opaque foreign type's instantiation clause declared more than once
   with the same foreign name and argument (see error_codes.ml). *)
val instance_duplicate : string

(* The `ctx` marker on a language block that is not a foreign handle's own
   method (see error_codes.ml for the rule). *)
val extern_ctx_on_free_call : string

(* A field's "= .field.method(args)" source names a method whose declared
   return is not the field's declared type. *)
val handle_call_type_mismatch : string

(* Register a code; raises [Invalid_argument] when it is already taken. Every
   constant above goes through this, so a collision fails at load time. *)
val register : string -> string

(* Every registered code, for the uniqueness/format test. *)
val registered : unit -> string list
val instance_names_mismatch : string

val extern_receiver_on_method : string
(** A call: line naming a type as receiver on a foreign handle's own method (the
    handle is already the receiver). *)

val extern_receiver_with_new : string
(** A call: line with a type receiver and the "new" marker at once. *)

val extern_type_arg_unknown : string
(** A call: argument passing a class reference that names no opaque handle of
    its own ext block. *)
