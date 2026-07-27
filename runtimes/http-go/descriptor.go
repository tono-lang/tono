// Package tonohttp is the hand-written HTTP transport runtime for
// tono-generated Go SDKs. It interprets the opaque wire descriptor: builds the
// request, performs the call, classifies the response. It ships no auth of its
// own; bespoke auth sets a header through Options.Headers or a hook.
package tonohttp

import (
	"encoding/json"
	"fmt"
)

// Part says where one input member goes in the HTTP request. Kind is one of
// "label" (substitutes {member-name} in the uri), "query", "header", "body"
// (a field inside the JSON body), or "payload" (the member is the whole body,
// no envelope). Name carries the query/header wire name.
type Part struct {
	Kind string `json:"kind"`
	Name string `json:"name"`
}

// Binding pairs an input member name with its Part. On the wire it is a
// two-element JSON array, mirroring the compiler's emission.
type Binding struct {
	Member string
	Part   Part
}

// UnmarshalJSON reads the compiler's ["member", {"kind": ...}] pair form.
func (b *Binding) UnmarshalJSON(data []byte) error {
	var pair [2]json.RawMessage
	if err := json.Unmarshal(data, &pair); err != nil {
		return fmt.Errorf("binding is not a two-element array: %w", err)
	}
	if err := json.Unmarshal(pair[0], &b.Member); err != nil {
		return fmt.Errorf("binding member: %w", err)
	}
	if err := json.Unmarshal(pair[1], &b.Part); err != nil {
		return fmt.Errorf("binding part: %w", err)
	}
	return nil
}

// ResponsePart says where one output member is read from in the HTTP response:
// Kind "header" (with Name) or "statusCode".
type ResponsePart struct {
	Kind string `json:"kind"`
	Name string `json:"name"`
}

// ResponseBinding pairs an output member name with its ResponsePart.
type ResponseBinding struct {
	Member string
	Part   ResponsePart
}

// UnmarshalJSON reads the ["member", {"kind": ...}] pair form.
func (b *ResponseBinding) UnmarshalJSON(data []byte) error {
	var pair [2]json.RawMessage
	if err := json.Unmarshal(data, &pair); err != nil {
		return fmt.Errorf("response binding is not a two-element array: %w", err)
	}
	if err := json.Unmarshal(pair[0], &b.Member); err != nil {
		return fmt.Errorf("response binding member: %w", err)
	}
	if err := json.Unmarshal(pair[1], &b.Part); err != nil {
		return fmt.Errorf("response binding part: %w", err)
	}
	return nil
}

// SuccessCase is one declared success status. The second wire element (the
// output type ref) is opaque to the runtime: the SDK owns decoding, the runtime
// uses only the status to decide success vs error.
type SuccessCase struct {
	Status int
}

// UnmarshalJSON reads the [status, typeRef] pair form, discarding the ref.
func (s *SuccessCase) UnmarshalJSON(data []byte) error {
	var pair []json.RawMessage
	if err := json.Unmarshal(data, &pair); err != nil || len(pair) < 1 {
		return fmt.Errorf("success case is not an array: %s", string(data))
	}
	if err := json.Unmarshal(pair[0], &s.Status); err != nil {
		return fmt.Errorf("success status: %w", err)
	}
	return nil
}

// DeclaredError is one declared error of the operation: its status, the error
// shape id, the optional discriminator value matched against the body's "code"
// field, and whether the error is retryable. The wire form is a three- or
// four-element array; the fourth element (retryable) is absent in descriptors
// emitted before retry existed, and absence means not retryable.
type DeclaredError struct {
	Status    int
	ID        string
	Code      *string
	Retryable bool
}

// UnmarshalJSON reads [status, id, code] or [status, id, code, retryable].
func (e *DeclaredError) UnmarshalJSON(data []byte) error {
	var parts []json.RawMessage
	if err := json.Unmarshal(data, &parts); err != nil || len(parts) < 3 {
		return fmt.Errorf("declared error is not a 3+ element array: %s", string(data))
	}
	if err := json.Unmarshal(parts[0], &e.Status); err != nil {
		return fmt.Errorf("declared error status: %w", err)
	}
	if err := json.Unmarshal(parts[1], &e.ID); err != nil {
		return fmt.Errorf("declared error id: %w", err)
	}
	if err := json.Unmarshal(parts[2], &e.Code); err != nil {
		return fmt.Errorf("declared error code: %w", err)
	}
	if len(parts) > 3 {
		if err := json.Unmarshal(parts[3], &e.Retryable); err != nil {
			return fmt.Errorf("declared error retryable: %w", err)
		}
	}
	return nil
}

// TemplatePart is one piece of a template position: a literal run, an
// entry-field placeholder resolved from Options.Values by its dotted path, or
// an operation-input placeholder resolved from the call's input record.
// Exactly one of the three is set; the wire form is {"lit"|"field"|"input": ...}.
type TemplatePart struct {
	Lit   *string  `json:"lit"`
	Field []string `json:"field"`
	Input *string  `json:"input"`
}

// ValueExpr is a descriptor value position: a literal frozen by the compiler,
// an entry-field reference resolved from Options.Values, or a template of
// parts. Exactly one of the three is set.
type ValueExpr struct {
	Lit      any            `json:"lit"`
	Field    []string       `json:"field"`
	Template []TemplatePart `json:"template"`
}

// RequestHeader is one declared request header: its name as a template and its
// value expression. On the wire it is a two-element JSON array.
type RequestHeader struct {
	Key   []TemplatePart
	Value ValueExpr
}

// UnmarshalJSON reads the compiler's [[part...], valueExpr] pair form.
func (h *RequestHeader) UnmarshalJSON(data []byte) error {
	var pair [2]json.RawMessage
	if err := json.Unmarshal(data, &pair); err != nil {
		return fmt.Errorf("request header is not a two-element array: %w", err)
	}
	if err := json.Unmarshal(pair[0], &h.Key); err != nil {
		return fmt.Errorf("request header key: %w", err)
	}
	if err := json.Unmarshal(pair[1], &h.Value); err != nil {
		return fmt.Errorf("request header value: %w", err)
	}
	return nil
}

// ValueSource is a descriptor position that yields a number at call time:
// either a literal frozen by the compiler (Lit) or a reference to a resolved
// client field looked up in Options.Values by canonical name (Ref).
type ValueSource struct {
	Ref *string  `json:"ref"`
	Lit *float64 `json:"lit"`
}

// RetrySpec declares that the operation retries, with the maximum number of
// retries (attempts after the first) read from Max.
type RetrySpec struct {
	Max ValueSource `json:"max"`
}

// WireDescriptor is the opaque, language-agnostic shape the tono HTTP Protocol
// produces for each operation and the generated SDK embeds verbatim. This
// runtime is the only layer that reads it; it executes the descriptor without
// re-deriving any default and raises no typed error (the generated SDK maps
// the Outcome onto its taxonomy).
type WireDescriptor struct {
	HTTPMethod       string            `json:"http_method"`
	URI              string            `json:"uri"`
	Bindings         []Binding         `json:"bindings"`
	ResponseBindings []ResponseBinding `json:"response_bindings"`
	Success          []SuccessCase     `json:"success"`
	Errors           []DeclaredError   `json:"errors"`
	// Retry is absent when the operation declares no retry: absent means one
	// attempt, ever. Timeout is the per-attempt budget in milliseconds; absent
	// means no per-attempt deadline.
	Retry   *RetrySpec   `json:"retry"`
	Timeout *ValueSource `json:"timeout"`
	// Endpoint names the resolved client field (by path) whose value is the
	// base URL for this operation; absent means Options.BaseURL. RequestHeaders
	// are the operation's declared headers, applied before the caller's
	// Options.Headers so a bespoke or caller-supplied header wins.
	Endpoint       []string        `json:"endpoint"`
	RequestHeaders []RequestHeader `json:"request_headers"`
}

// ParseDescriptor decodes the JSON descriptor literal embedded by the
// generated SDK.
func ParseDescriptor(data []byte) (*WireDescriptor, error) {
	var d WireDescriptor
	if err := json.Unmarshal(data, &d); err != nil {
		return nil, fmt.Errorf("wire descriptor: %w", err)
	}
	return &d, nil
}
