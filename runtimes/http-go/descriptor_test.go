package tonohttp

import (
	"strings"
	"testing"
)

// The exact literal the compiler embeds in a generated SDK (taken from the
// payments example): parsing it is the contract with the emitter.
const embeddedDescriptor = `{"bindings":[["id",{"kind":"body"}],["amount",{"kind":"body"}]],"errors":[[402,"payments.charges#card_declined","card_declined"],[404,"payments.charges#not_found",null]],"http_method":"POST","response_bindings":[],"success":[[200,{"args":[],"ref":"payments.charges#charge"}]],"uri":"/charges"}`

func TestParseEmbeddedDescriptor(t *testing.T) {
	d, err := ParseDescriptor([]byte(embeddedDescriptor))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	if d.HTTPMethod != "POST" || d.URI != "/charges" {
		t.Fatalf("method/uri: %q %q", d.HTTPMethod, d.URI)
	}
	if len(d.Bindings) != 2 || d.Bindings[0].Member != "id" || d.Bindings[0].Part.Kind != "body" {
		t.Fatalf("bindings: %+v", d.Bindings)
	}
	if len(d.Success) != 1 || d.Success[0].Status != 200 {
		t.Fatalf("success: %+v", d.Success)
	}
	if len(d.Errors) != 2 {
		t.Fatalf("errors: %+v", d.Errors)
	}
	declined := d.Errors[0]
	if declined.Status != 402 || declined.ID != "payments.charges#card_declined" {
		t.Fatalf("first error: %+v", declined)
	}
	if declined.Code == nil || *declined.Code != "card_declined" {
		t.Fatalf("first error code: %+v", declined.Code)
	}
	// A three-element error (emitted before retry existed) is not retryable.
	if declined.Retryable {
		t.Fatal("three-element error parsed as retryable")
	}
	if d.Errors[1].Code != nil {
		t.Fatalf("null code parsed as: %+v", d.Errors[1].Code)
	}
	if d.Retry != nil || d.Timeout != nil {
		t.Fatalf("absent retry/timeout parsed as present: %+v %+v", d.Retry, d.Timeout)
	}
}

func TestParseRetryTimeoutAndRetryableFlag(t *testing.T) {
	raw := `{
		"http_method": "POST", "uri": "/x", "bindings": [],
		"response_bindings": [["requestId", {"kind": "header", "name": "X-Request-Id"}], ["httpStatus", {"kind": "statusCode"}]],
		"success": [[200, null]],
		"errors": [[429, "svc#overloaded", "overloaded", true], [404, "svc#not_found", null, false]],
		"retry": {"max": {"ref": "max_retries"}},
		"timeout": {"lit": 5000}
	}`
	d, err := ParseDescriptor([]byte(raw))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	if len(d.ResponseBindings) != 2 || d.ResponseBindings[0].Member != "requestId" ||
		d.ResponseBindings[0].Part.Name != "X-Request-Id" || d.ResponseBindings[1].Part.Kind != "statusCode" {
		t.Fatalf("response bindings: %+v", d.ResponseBindings)
	}
	if !d.Errors[0].Retryable || d.Errors[1].Retryable {
		t.Fatalf("retryable flags: %+v", d.Errors)
	}
	if d.Retry == nil || d.Retry.Max.Ref == nil || *d.Retry.Max.Ref != "max_retries" || d.Retry.Max.Lit != nil {
		t.Fatalf("retry: %+v", d.Retry)
	}
	if d.Timeout == nil || d.Timeout.Lit == nil || *d.Timeout.Lit != 5000 || d.Timeout.Ref != nil {
		t.Fatalf("timeout: %+v", d.Timeout)
	}
}

func TestParseSuccessCaseKeepsOnlyTheStatus(t *testing.T) {
	// The type-ref element is opaque and optional to the runtime: a bare
	// [status] entry parses too.
	d, err := ParseDescriptor([]byte(`{"success": [[201]]}`))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	if len(d.Success) != 1 || d.Success[0].Status != 201 {
		t.Fatalf("success: %+v", d.Success)
	}
}

func TestParseRejectsMalformedShapes(t *testing.T) {
	cases := map[string]string{
		"not json":                 `nope`,
		"binding not a pair":       `{"bindings": ["id"]}`,
		"binding member not a str": `{"bindings": [[1, {"kind": "body"}]]}`,
		"binding part not object":  `{"bindings": [["id", 3]]}`,
		"response binding scalar":  `{"response_bindings": [5]}`,
		"response member not str":  `{"response_bindings": [[5, {"kind": "statusCode"}]]}`,
		"response part not object": `{"response_bindings": [["m", 5]]}`,
		"success not array":        `{"success": [200]}`,
		"success status not int":   `{"success": [["x", null]]}`,
		"error too short":          `{"errors": [[402, "id"]]}`,
		"error status not int":     `{"errors": [["x", "id", null]]}`,
		"error id not string":      `{"errors": [[402, 5, null]]}`,
		"error code not string":    `{"errors": [[402, "id", 5]]}`,
		"error retryable not bool": `{"errors": [[402, "id", null, "yes"]]}`,
	}
	for name, raw := range cases {
		if _, err := ParseDescriptor([]byte(raw)); err == nil {
			t.Errorf("%s: parsed without error", name)
		}
	}
}

func TestRequestHeaderParseErrors(t *testing.T) {
	// The pair form is [[part...], valueExpr]; each malformed position names
	// its place in the error.
	cases := []struct {
		name string
		blob string
		want string
	}{
		{"not an array", `{"request_headers": [42]}`, "two-element array"},
		{"bad key", `{"request_headers": [[42, {"lit": "v"}]]}`, "request header key"},
		{"bad value", `{"request_headers": [[[{"lit": "K"}], 42]]}`, "request header value"},
	}
	for _, c := range cases {
		d := `{"http_method": "GET", "uri": "/", ` + c.blob[1:]
		if _, err := ParseDescriptor([]byte(d)); err == nil || !strings.Contains(err.Error(), c.want) {
			t.Fatalf("%s: %v", c.name, err)
		}
	}
}

func TestRequestHeaderParsesThePairForm(t *testing.T) {
	d, err := ParseDescriptor([]byte(`{"http_method": "GET", "uri": "/", "request_headers": [[[{"lit": "K"}], {"field": ["token"]}]], "endpoint": ["endpoint"]}`))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	if len(d.RequestHeaders) != 1 || d.RequestHeaders[0].Key[0].Lit == nil || *d.RequestHeaders[0].Key[0].Lit != "K" {
		t.Fatalf("key: %+v", d.RequestHeaders)
	}
	if d.RequestHeaders[0].Value.Field == nil || d.RequestHeaders[0].Value.Field[0] != "token" {
		t.Fatalf("value: %+v", d.RequestHeaders)
	}
	if len(d.Endpoint) != 1 || d.Endpoint[0] != "endpoint" {
		t.Fatalf("endpoint: %+v", d.Endpoint)
	}
}
