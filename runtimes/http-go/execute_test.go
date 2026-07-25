package tonohttp

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func newRuntime(t *testing.T, opts Options) *Runtime {
	t.Helper()
	r, err := New(opts)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	return r
}

// A recording transport: captures every request and answers from a fixed
// response, so a test can assert exactly what the runtime built.
func recorder(status int, body string, headers map[string]string) (Transport, *[]CanonicalRequest) {
	var calls []CanonicalRequest
	transport := func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
		calls = append(calls, req)
		return CanonicalResponse{Status: status, Headers: headers, Body: body}, nil
	}
	return transport, &calls
}

func TestNewRejectsBothTransportSlots(t *testing.T) {
	transport, _ := recorder(200, "{}", nil)
	if _, err := New(Options{Client: &http.Client{}, Transport: transport}); err == nil {
		t.Fatal("both slots set must be a construction error")
	} else if !strings.Contains(err.Error(), "mutually exclusive") {
		t.Fatalf("error must name the conflict: %v", err)
	}
	if _, err := New(Options{Client: &http.Client{}}); err != nil {
		t.Fatalf("native slot alone: %v", err)
	}
	if _, err := New(Options{Transport: transport}); err != nil {
		t.Fatalf("canonical slot alone: %v", err)
	}
	if _, err := New(Options{}); err != nil {
		t.Fatalf("neither slot (default client): %v", err)
	}
}

func TestCanonicalTransportReceivesTheBuiltRequest(t *testing.T) {
	transport, calls := recorder(200, "{}", nil)
	r := newRuntime(t, Options{
		BaseURL:   "https://api.test",
		Transport: transport,
		Headers:   map[string]string{"Authorization": "Bearer t"},
	})
	d := desc(func(d *WireDescriptor) {
		d.HTTPMethod = "PUT"
		d.URI = "/x/{id}"
		d.Bindings = []Binding{
			binding("id", "label", ""),
			binding("q", "query", "q"),
			binding("h", "header", "H"),
			binding("field", "body", ""),
		}
	})
	outcome, err := r.Execute(context.Background(), d, map[string]any{"id": "1", "q": "2", "h": "3", "field": "4"}, nil)
	if err != nil || outcome.Kind != OutcomeSuccess {
		t.Fatalf("outcome: %+v %v", outcome, err)
	}
	if len(*calls) != 1 {
		t.Fatalf("attempts: %d", len(*calls))
	}
	req := (*calls)[0]
	if req.Method != "PUT" || req.URL != "https://api.test/x/1?q=2" {
		t.Fatalf("request line: %s %s", req.Method, req.URL)
	}
	if req.Headers["H"] != "3" || req.Headers["Authorization"] != "Bearer t" {
		t.Fatalf("headers: %+v", req.Headers)
	}
	if string(req.Body) != `{"field":"4"}` {
		t.Fatalf("body: %s", req.Body)
	}
	if req.Headers["content-type"] != "application/json" {
		t.Fatalf("default content-type missing: %+v", req.Headers)
	}
}

func TestContentTypeDefault(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("a", "body", "")}
	})

	transport, calls := recorder(200, "{}", nil)
	r := newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
	if _, err := r.Execute(context.Background(), d, map[string]any{}, nil); err != nil {
		t.Fatal(err)
	}
	if _, ok := (*calls)[0].Headers["content-type"]; ok || (*calls)[0].Body != nil {
		t.Fatalf("no body must mean no content-type: %+v", (*calls)[0])
	}

	transport, calls = recorder(200, "{}", nil)
	r = newRuntime(t, Options{
		BaseURL:   "https://api.test",
		Transport: transport,
		Headers:   map[string]string{"Content-Type": "application/vnd.api+json"},
	})
	if _, err := r.Execute(context.Background(), d, map[string]any{"a": float64(1)}, nil); err != nil {
		t.Fatal(err)
	}
	headers := (*calls)[0].Headers
	if headers["Content-Type"] != "application/vnd.api+json" {
		t.Fatalf("caller content-type lost: %+v", headers)
	}
	if _, ok := headers["content-type"]; ok {
		t.Fatalf("second content-type added beside the caller's: %+v", headers)
	}
}

func TestNativeSlotAgainstARealServer(t *testing.T) {
	var seen *http.Request
	var seenBody string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		seen = req.Clone(context.Background())
		data, _ := io.ReadAll(req.Body)
		seenBody = string(data)
		w.Header().Set("X-Request-Id", "req-1")
		w.WriteHeader(200)
		_, _ = w.Write([]byte(`{"id":"x"}`))
	}))
	defer server.Close()

	// Client nil exercises the http.DefaultClient fallback.
	r := newRuntime(t, Options{BaseURL: server.URL})
	d := desc(func(d *WireDescriptor) {
		d.URI = "/things/{id}"
		d.Bindings = []Binding{
			binding("id", "label", ""),
			binding("q", "query", "q"),
			binding("trace", "header", "X-Trace"),
			binding("a", "body", ""),
		}
		d.ResponseBindings = []ResponseBinding{{Member: "requestId", Part: ResponsePart{Kind: "header", Name: "X-Request-Id"}}}
	})
	outcome, err := r.Execute(context.Background(), d, map[string]any{"id": "7", "q": "v", "trace": "t1", "a": float64(1)}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if outcome.Kind != OutcomeSuccess || outcome.Status != 200 {
		t.Fatalf("outcome: %+v", outcome)
	}
	// The response-bound header folds into the body under its member name.
	if outcome.Body != `{"id":"x","requestId":"req-1"}` {
		t.Fatalf("folded body: %s", outcome.Body)
	}
	if seen.Method != "POST" || seen.URL.Path != "/things/7" || seen.URL.RawQuery != "q=v" {
		t.Fatalf("request line: %s %s?%s", seen.Method, seen.URL.Path, seen.URL.RawQuery)
	}
	if seen.Header.Get("X-Trace") != "t1" || seen.Header.Get("Content-Type") != "application/json" {
		t.Fatalf("headers: %+v", seen.Header)
	}
	if seenBody != `{"a":1}` {
		t.Fatalf("body: %s", seenBody)
	}
}

func TestNativeTransportFailures(t *testing.T) {
	r := newRuntime(t, Options{BaseURL: "http://127.0.0.1:0"})
	outcome, err := r.Execute(context.Background(), desc(nil), nil, nil)
	if err != nil || outcome.Kind != OutcomeTransport || outcome.Cause == nil {
		t.Fatalf("network failure must be a transport outcome: %+v %v", outcome, err)
	}

	// An invalid method fails request construction, which is transport too.
	d := desc(func(d *WireDescriptor) { d.HTTPMethod = "BAD METHOD" })
	outcome, err = r.Execute(context.Background(), d, nil, nil)
	if err != nil || outcome.Kind != OutcomeTransport {
		t.Fatalf("bad method: %+v %v", outcome, err)
	}
}

func TestNativeBodyReadFailureIsTransport(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		// Promise more bytes than are sent, then cut the connection: the body
		// read fails mid-stream.
		w.Header().Set("Content-Length", "100")
		_, _ = w.Write([]byte("short"))
		if f, ok := w.(http.Flusher); ok {
			f.Flush()
		}
		hj, ok := w.(http.Hijacker)
		if !ok {
			return
		}
		conn, _, _ := hj.Hijack()
		conn.Close()
	}))
	defer server.Close()
	r := newRuntime(t, Options{BaseURL: server.URL, Client: server.Client()})
	outcome, err := r.Execute(context.Background(), desc(nil), nil, nil)
	if err != nil || outcome.Kind != OutcomeTransport || outcome.Cause == nil {
		t.Fatalf("body-read failure must be a transport outcome: %+v %v", outcome, err)
	}
}

func TestSuccessClassification(t *testing.T) {
	run := func(status int, mutate func(*WireDescriptor)) Outcome {
		transport, _ := recorder(status, "{}", nil)
		r := newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
		outcome, err := r.Execute(context.Background(), desc(mutate), nil, nil)
		if err != nil {
			t.Fatal(err)
		}
		return outcome
	}
	if run(299, nil).Kind != OutcomeSuccess {
		t.Fatal("299 is a 2xx success")
	}
	if run(300, nil).Kind != OutcomeError {
		t.Fatal("300 is not a success")
	}
	if run(150, nil).Kind != OutcomeError {
		t.Fatal("a sub-200 status is not a success on its own")
	}
	if run(200, func(d *WireDescriptor) { d.Success = []SuccessCase{{Status: 201}} }).Kind != OutcomeSuccess {
		t.Fatal("any 2xx succeeds even when another code was declared")
	}
	declared := func(d *WireDescriptor) { d.Success = []SuccessCase{{Status: 301}, {Status: 302}} }
	if run(302, declared).Kind != OutcomeSuccess {
		t.Fatal("a declared non-2xx code succeeds")
	}
	if run(418, declared).Kind != OutcomeError {
		t.Fatal("an undeclared non-2xx code errors")
	}
}

func TestResponseBindings(t *testing.T) {
	run := func(body string, headers map[string]string, mutate func(*WireDescriptor)) Outcome {
		transport, _ := recorder(200, body, headers)
		r := newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
		outcome, err := r.Execute(context.Background(), desc(mutate), nil, nil)
		if err != nil {
			t.Fatal(err)
		}
		return outcome
	}
	headerBinding := func(d *WireDescriptor) {
		d.ResponseBindings = []ResponseBinding{{Member: "requestId", Part: ResponsePart{Kind: "header", Name: "X-Request-Id"}}}
	}
	// Canonical response headers are lowercased by contract; the lookup is too.
	got := run(`{"id":"x"}`, map[string]string{"x-request-id": "r1"}, headerBinding)
	if got.Body != `{"id":"x","requestId":"r1"}` {
		t.Fatalf("header fold: %s", got.Body)
	}
	got = run("", map[string]string{"x-request-id": "r1"}, headerBinding)
	if got.Body != `{"requestId":"r1"}` {
		t.Fatalf("bound field must stand alone on an empty body: %s", got.Body)
	}
	got = run("not json", nil, headerBinding)
	if got.Body != `{"requestId":null}` {
		t.Fatalf("missing header folds as null, non-JSON body dropped: %s", got.Body)
	}
	got = run(`{"id":"x"}`, nil, func(d *WireDescriptor) {
		d.ResponseBindings = []ResponseBinding{{Member: "httpStatus", Part: ResponsePart{Kind: "statusCode"}}}
	})
	if got.Body != `{"httpStatus":200,"id":"x"}` {
		t.Fatalf("status fold: %s", got.Body)
	}
	// A spaced body proves the text passes through verbatim when there is
	// nothing to fold in.
	got = run(`{"id": "x"}`, nil, nil)
	if got.Body != `{"id": "x"}` {
		t.Fatalf("verbatim body: %s", got.Body)
	}
}

func TestHooks(t *testing.T) {
	transport, calls := recorder(200, "{}", nil)
	r := newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
	hooks := &Hooks{
		BeforeRequest: func(ctx context.Context, req CanonicalRequest) (CanonicalRequest, error) {
			req.Headers["Authorization"] = "signed"
			return req, nil
		},
	}
	if _, err := r.Execute(context.Background(), desc(nil), nil, hooks); err != nil {
		t.Fatal(err)
	}
	if (*calls)[0].Headers["Authorization"] != "signed" {
		t.Fatalf("BeforeRequest rewrite lost: %+v", (*calls)[0].Headers)
	}

	transport, _ = recorder(500, "{}", nil)
	r = newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
	rewrite := &Hooks{
		AfterResponse: func(ctx context.Context, res CanonicalResponse) (CanonicalResponse, error) {
			res.Status = 200
			return res, nil
		},
	}
	outcome, err := r.Execute(context.Background(), desc(nil), nil, rewrite)
	if err != nil || outcome.Kind != OutcomeSuccess {
		t.Fatalf("classification must read the post-hook response: %+v %v", outcome, err)
	}
}

func TestHookErrorsPropagateRawAndNeverRetry(t *testing.T) {
	boom := errors.New("hook broke")
	withRetry := func(d *WireDescriptor) { d.Retry = &RetrySpec{Max: ValueSource{Lit: lit(3)}} }

	transport, calls := recorder(200, "{}", nil)
	r := newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
	_, err := r.Execute(context.Background(), desc(withRetry), nil, &Hooks{
		BeforeRequest: func(ctx context.Context, req CanonicalRequest) (CanonicalRequest, error) {
			return req, boom
		},
	})
	if !errors.Is(err, boom) || len(*calls) != 0 {
		t.Fatalf("BeforeRequest error must propagate before any attempt: %v, %d calls", err, len(*calls))
	}

	transport, calls = recorder(503, "{}", nil)
	r = newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
	_, err = r.Execute(context.Background(), desc(func(d *WireDescriptor) {
		withRetry(d)
		d.Errors = []DeclaredError{{Status: 503, ID: "svc#unavailable", Retryable: true}}
	}), nil, &Hooks{
		AfterResponse: func(ctx context.Context, res CanonicalResponse) (CanonicalResponse, error) {
			return res, boom
		},
	})
	if !errors.Is(err, boom) || len(*calls) != 1 {
		t.Fatalf("AfterResponse error must propagate, not retry: %v, %d calls", err, len(*calls))
	}
}

func TestUnencodableInputSurfacesAsAnError(t *testing.T) {
	transport, calls := recorder(200, "{}", nil)
	r := newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("a", "body", "")}
	})
	if _, err := r.Execute(context.Background(), d, map[string]any{"a": make(chan int)}, nil); err == nil {
		t.Fatal("unencodable input must error, not call the transport")
	}
	if len(*calls) != 0 {
		t.Fatalf("transport called %d times", len(*calls))
	}
}

func TestTimeoutDeadlineIsSetOnlyWhenDeclared(t *testing.T) {
	var hadDeadline bool
	transport := func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
		_, hadDeadline = ctx.Deadline()
		return CanonicalResponse{Status: 200, Body: "{}"}, nil
	}
	r := newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
	if _, err := r.Execute(context.Background(), desc(nil), nil, nil); err != nil {
		t.Fatal(err)
	}
	if hadDeadline {
		t.Fatal("no declared timeout must mean no deadline")
	}
	d := desc(func(d *WireDescriptor) { d.Timeout = &ValueSource{Lit: lit(5000)} })
	if _, err := r.Execute(context.Background(), d, nil, nil); err != nil {
		t.Fatal(err)
	}
	if !hadDeadline {
		t.Fatal("declared timeout must set a deadline")
	}
}
