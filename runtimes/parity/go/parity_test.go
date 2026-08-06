// The cross-runtime parity suite, Go side: like the TypeScript harness (and
// unlike Rust, still exercising the hand-written runtime package directly),
// this one drives the real generated SDK compiled from ../spec.tono. This
// file is not run in place: scripts/run-parity.sh copies it into the SDK's
// own generated package directory (and ../vectors.json next to the go.mod)
// before running `go test`, so the same-package references below resolve
// against generated code, not against this source tree. It lives here, next
// to the spec and vectors it exercises, rather than inside runtimes/http-go:
// that package (and its Rust counterpart) is retired once every target emits
// its own transport, and this harness outlives both.
//
// Jitter is pinned to 0.5 and backoff sleeps are recorded through the
// generated client's own unexported timing seam (c.timing), reachable only
// because this file joins the generated package. The per-attempt @timeout
// still fires as a real context deadline - only the retry backoff is mocked -
// so a "hang" vector costs a few real milliseconds, not the seconds a naive
// backoff would otherwise take.
package parity

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"reflect"
	"testing"
	"time"

	"parity.test/sdk/internal/transport"
	"parity.test/sdk/support"
)

type parityVector struct {
	Name   string         `json:"name"`
	Op     string         `json:"op"`
	Config parityConfig   `json:"config"`
	Input  map[string]any `json:"input"`
	Script []parityStep   `json:"script"`
	Expect parityExpect   `json:"expect"`
}

// parityConfig is real client construction: max_retries feeds WithMaxRetries,
// timeout_ms feeds WithTimeout as a millisecond duration literal. The
// vectors' declared_errors field is not read here: the generated SDK decides
// retryability for real, from spec.tono's own @errorCode/@retryable
// declarations.
type parityConfig struct {
	MaxRetries *float64 `json:"max_retries"`
	TimeoutMs  *float64 `json:"timeout_ms"`
}

type parityStep struct {
	Kind   string `json:"kind"`
	Status int    `json:"status"`
	Body   string `json:"body"`
}

type parityExpect struct {
	Outcome  string    `json:"outcome"`
	Status   int       `json:"status"`
	Body     string    `json:"body"`
	Attempts int       `json:"attempts"`
	DelaysMs []float64 `json:"delays_ms"`
}

func loadParityVectors(t *testing.T) []parityVector {
	t.Helper()
	// run-parity.sh drops the vectors next to the generated go.mod; this file
	// joins the module's own package one directory down.
	data, err := os.ReadFile("../vectors.json")
	if err != nil {
		t.Fatalf("parity vectors: %v", err)
	}
	var file struct {
		Vectors []parityVector `json:"vectors"`
	}
	if err := json.Unmarshal(data, &file); err != nil {
		t.Fatalf("parity vectors: %v", err)
	}
	if len(file.Vectors) == 0 {
		t.Fatal("parity vectors file has no vectors")
	}
	return file.Vectors
}

// scriptedTransport answers one canned step per attempt: response returns
// that status/body, transport_failure fails the attempt, hang honors the
// per-attempt ctx deadline (with a bail-out so a mutant that breaks the
// deadline fails the vector fast instead of hanging the binary).
func scriptedTransport(t *testing.T, script []parityStep, attempts *int, requests *[]support.HTTPRequest) support.HTTPTransport {
	return func(ctx context.Context, req support.HTTPRequest) (support.HTTPResponse, error) {
		if *attempts >= len(script) {
			t.Fatalf("transport called %d times for a %d-step script", *attempts+1, len(script))
		}
		*requests = append(*requests, req)
		step := script[*attempts]
		*attempts++
		switch step.Kind {
		case "response":
			return support.HTTPResponse{Status: step.Status, Body: step.Body}, nil
		case "transport_failure":
			return support.HTTPResponse{}, errors.New("scripted transport failure")
		case "hang":
			select {
			case <-ctx.Done():
				return support.HTTPResponse{}, ctx.Err()
			case <-time.After(2 * time.Second):
				return support.HTTPResponse{}, errors.New("hang step: the per-attempt deadline never fired")
			}
		default:
			return support.HTTPResponse{}, fmt.Errorf("unknown script step %q", step.Kind)
		}
	}
}

func clientFor(t *testing.T, vector parityVector, scripted support.HTTPTransport, delays *[]time.Duration) *Client {
	t.Helper()
	var opts []ClientOption
	if vector.Config.MaxRetries != nil {
		opts = append(opts, WithMaxRetries(int32(*vector.Config.MaxRetries)))
	}
	if vector.Config.TimeoutMs != nil {
		opts = append(opts, WithTimeout(support.Duration(fmt.Sprintf("%gms", *vector.Config.TimeoutMs))))
	}
	c, err := newWithTransport(scripted, "https://api.test", opts...)
	if err != nil {
		t.Fatalf("construct client: %v", err)
	}
	// The timing seam is an unexported client field, assignable only because
	// this test joins the generated package; a consumer of the SDK has no way
	// to reach it.
	c.timing = transport.Timing{
		Sleep: func(ctx context.Context, d time.Duration) error {
			*delays = append(*delays, d)
			return nil
		},
		Random: func() float64 { return 0.5 },
	}
	return c
}

func callOp(t *testing.T, c *Client, op string) (Thing, error) {
	t.Helper()
	switch op {
	case "retrying":
		return c.Retrying(context.Background(), Thing{})
	case "retrying_with_timeout":
		return c.RetryingWithTimeout(context.Background(), Thing{})
	case "timeout_only":
		return c.TimeoutOnly(context.Background(), Thing{})
	default:
		t.Fatalf("unknown parity op %q", op)
		return Thing{}, nil
	}
}

// wantError rebuilds the exact error the vector expects through the
// operation's own discriminator, so the comparison covers both the typed
// declared errors and the APIError fallback without the harness re-deriving
// either. Only `retrying` declares errors; the other ops always fall back.
func wantError(op string, status int, body string) error {
	if op == "retrying" {
		return DecodeRetryingError(status, []byte(body))
	}
	return &APIError{Status: status, Body: body}
}

func TestParityVectors(t *testing.T) {
	for _, vector := range loadParityVectors(t) {
		t.Run(vector.Name, func(t *testing.T) {
			attempts := 0
			var requests []support.HTTPRequest
			var delays []time.Duration
			scripted := scriptedTransport(t, vector.Script, &attempts, &requests)
			c := clientFor(t, vector, scripted, &delays)
			got, err := callOp(t, c, vector.Op)
			switch vector.Expect.Outcome {
			case "success":
				if err != nil {
					t.Fatalf("outcome: %v, want success", err)
				}
				encoded, marshalErr := json.Marshal(got)
				if marshalErr != nil {
					t.Fatalf("re-encode output: %v", marshalErr)
				}
				var gotBody, wantBody any
				if err := json.Unmarshal(encoded, &gotBody); err != nil {
					t.Fatalf("parse re-encoded output: %v", err)
				}
				if err := json.Unmarshal([]byte(vector.Expect.Body), &wantBody); err != nil {
					t.Fatalf("parse expected body: %v", err)
				}
				if !reflect.DeepEqual(gotBody, wantBody) {
					t.Fatalf("body: %s, want %s", encoded, vector.Expect.Body)
				}
			case "transport":
				var te *TransportError
				if !errors.As(err, &te) {
					t.Fatalf("outcome: %v, want a transport failure", err)
				}
				if te.Cause == nil {
					t.Fatal("transport outcome without a cause")
				}
			case "error":
				want := wantError(vector.Op, vector.Expect.Status, vector.Expect.Body)
				if !reflect.DeepEqual(err, want) {
					t.Fatalf("error: %#v, want %#v", err, want)
				}
			default:
				t.Fatalf("unknown expected outcome %q", vector.Expect.Outcome)
			}
			if attempts != vector.Expect.Attempts {
				t.Fatalf("attempts: %d, want %d", attempts, vector.Expect.Attempts)
			}
			if len(requests) != vector.Expect.Attempts {
				t.Fatalf("recorded requests: %d, want %d", len(requests), vector.Expect.Attempts)
			}
			if len(delays) != len(vector.Expect.DelaysMs) {
				t.Fatalf("delays: %v, want %v", delays, vector.Expect.DelaysMs)
			}
			for i, ms := range vector.Expect.DelaysMs {
				if want := time.Duration(ms * float64(time.Millisecond)); delays[i] != want {
					t.Fatalf("delay %d: %v, want %v", i, delays[i], want)
				}
			}
		})
	}
}
