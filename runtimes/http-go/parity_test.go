package tonohttp

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"testing"
	"time"
)

// The cross-runtime parity suite: the same behavior vectors run against every
// HTTP runtime, so retry/timeout/error handling cannot drift between them.
// Jitter is pinned to 0.5 and backoff sleeps are recorded, matching the other
// runtimes' harnesses.

type parityVector struct {
	Name           string          `json:"name"`
	Op             string          `json:"op"`
	Config         parityConfig    `json:"config"`
	DeclaredErrors []parityDeclErr `json:"declared_errors"`
	Input          map[string]any  `json:"input"`
	Script         []parityStep    `json:"script"`
	Expect         parityExpect    `json:"expect"`
}

// parityConfig is real client construction for the TypeScript harness (which
// drives a generated SDK directly) and, here, the literal retry/timeout
// values a synthetic descriptor is built from.
type parityConfig struct {
	MaxRetries *float64 `json:"max_retries"`
	TimeoutMs  *float64 `json:"timeout_ms"`
}

// descriptorFor rebuilds the synthetic WireDescriptor a vector's config
// describes: a retry key only when the vector sets max_retries, a timeout
// key only when it sets timeout_ms. Which fields are set already says
// everything spec.tono's op declares (a @retry field or a @timeout field),
// so this needs no per-op knowledge and nothing here changes when an op is
// added, renamed, or removed there. Runtime/Execute are exercised exactly
// as before; only the descriptor's origin moves from "parsed off JSON" to
// "built from config," since the JSON vector no longer carries one directly.
func descriptorFor(config parityConfig) json.RawMessage {
	base := map[string]any{
		"http_method":       "POST",
		"uri":               "/x",
		"bindings":          []any{},
		"response_bindings": []any{},
		"success":           []any{[]any{200, nil}},
	}
	if config.MaxRetries != nil {
		base["retry"] = map[string]any{"max": map[string]any{"lit": *config.MaxRetries}}
	}
	if config.TimeoutMs != nil {
		base["timeout"] = map[string]any{"lit": *config.TimeoutMs}
	}
	data, err := json.Marshal(base)
	if err != nil {
		panic(fmt.Sprintf("descriptorFor: %v", err))
	}
	return data
}

// parityDeclErr is one [status, code|null, retryable] triple: the fixture's
// stand-in for what a generated Decode<Op>Error + Retryable() pair would
// classify, without depending on any generated code to exist.
type parityDeclErr struct {
	Status    int
	Code      *string
	Retryable bool
}

func (e *parityDeclErr) UnmarshalJSON(data []byte) error {
	var parts [3]json.RawMessage
	if err := json.Unmarshal(data, &parts); err != nil {
		return fmt.Errorf("declared_errors entry is not a 3-element array: %w", err)
	}
	if err := json.Unmarshal(parts[0], &e.Status); err != nil {
		return fmt.Errorf("declared_errors status: %w", err)
	}
	if err := json.Unmarshal(parts[1], &e.Code); err != nil {
		return fmt.Errorf("declared_errors code: %w", err)
	}
	if err := json.Unmarshal(parts[2], &e.Retryable); err != nil {
		return fmt.Errorf("declared_errors retryable: %w", err)
	}
	return nil
}

// retryable builds the predicate Execute expects, exactly as a generated
// client would build it from its own Decode<Op>Error + Retryable() pair. A
// vector with no declared errors passes nil (never retryable).
func (v parityVector) retryable() func(status int, body string) bool {
	if len(v.DeclaredErrors) == 0 {
		return nil
	}
	rules := make([]declaredErrorRule, len(v.DeclaredErrors))
	for i, e := range v.DeclaredErrors {
		rules[i] = declaredErrorRule{Status: e.Status, Code: e.Code, Retryable: e.Retryable}
	}
	return classifyLikeDiscriminator(rules)
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
	// Assertions on the first request the runtime built: its full URL and a
	// subset of its headers. Optional; vectors that exercise the entry-scoped
	// descriptor positions (endpoint, request_headers, {.field} paths) use it.
	Request *parityRequest `json:"request"`
}

type parityRequest struct {
	URL     string            `json:"url"`
	Headers map[string]string `json:"headers"`
}

// loadParityVectors reads the shared vectors file. TONO_PARITY_VECTORS
// overrides the location and makes absence a failure; without it, a missing
// file skips (the mutation tool copies only this module, and the executor is
// pinned by the in-module tests).
func loadParityVectors(t *testing.T) []parityVector {
	t.Helper()
	path := os.Getenv("TONO_PARITY_VECTORS")
	required := path != ""
	if !required {
		path = "../parity/vectors.json"
	}
	data, err := os.ReadFile(path)
	if err != nil {
		if required {
			t.Fatalf("TONO_PARITY_VECTORS set but unreadable: %v", err)
		}
		t.Skipf("parity vectors not present at %s: %v", path, err)
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

func scriptedTransport(t *testing.T, script []parityStep, attempts *int, requests *[]CanonicalRequest) Transport {
	return func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
		if *attempts >= len(script) {
			t.Fatalf("transport called %d times for a %d-step script", *attempts+1, len(script))
		}
		*requests = append(*requests, req)
		step := script[*attempts]
		*attempts++
		switch step.Kind {
		case "response":
			return CanonicalResponse{Status: step.Status, Body: step.Body}, nil
		case "transport_failure":
			return CanonicalResponse{}, errors.New("scripted transport failure")
		case "hang":
			// Honor the ctx deadline, as the canonical transport contract
			// asks. The bail-out keeps a mutant that breaks the deadline from
			// hanging the whole test binary: it fails the vector fast instead.
			select {
			case <-ctx.Done():
				return CanonicalResponse{}, ctx.Err()
			case <-time.After(2 * time.Second):
				return CanonicalResponse{}, errors.New("hang step: the per-attempt deadline never fired")
			}
		default:
			return CanonicalResponse{}, fmt.Errorf("unknown script step %q", step.Kind)
		}
	}
}

func TestParityVectors(t *testing.T) {
	for _, vector := range loadParityVectors(t) {
		vector := vector
		t.Run(vector.Name, func(t *testing.T) {
			d, err := ParseDescriptor(descriptorFor(vector.Config))
			if err != nil {
				t.Fatalf("descriptor: %v", err)
			}
			attempts := 0
			var requests []CanonicalRequest
			r, delays := deterministic(t, Options{
				BaseURL:   "https://api.test",
				Transport: scriptedTransport(t, vector.Script, &attempts, &requests),
			})
			outcome, err := r.Execute(context.Background(), d, vector.Input, nil, vector.retryable())
			if err != nil {
				t.Fatalf("execute: %v", err)
			}
			if string(outcome.Kind) != vector.Expect.Outcome {
				t.Fatalf("outcome: %s, want %s", outcome.Kind, vector.Expect.Outcome)
			}
			if outcome.Kind == OutcomeTransport {
				if outcome.Cause == nil {
					t.Fatal("transport outcome without a cause")
				}
			} else if outcome.Status != vector.Expect.Status || outcome.Body != vector.Expect.Body {
				t.Fatalf("status/body: %d %q, want %d %q", outcome.Status, outcome.Body, vector.Expect.Status, vector.Expect.Body)
			}
			if attempts != vector.Expect.Attempts {
				t.Fatalf("attempts: %d, want %d", attempts, vector.Expect.Attempts)
			}
			if len(*delays) != len(vector.Expect.DelaysMs) {
				t.Fatalf("delays: %v, want %v", *delays, vector.Expect.DelaysMs)
			}
			for i, ms := range vector.Expect.DelaysMs {
				if want := time.Duration(ms * float64(time.Millisecond)); (*delays)[i] != want {
					t.Fatalf("delay %d: %v, want %v", i, (*delays)[i], want)
				}
			}
			if want := vector.Expect.Request; want != nil {
				if len(requests) == 0 {
					t.Fatal("request expectation with no recorded request")
				}
				first := requests[0]
				if want.URL != "" && first.URL != want.URL {
					t.Fatalf("request url: %q, want %q", first.URL, want.URL)
				}
				for name, value := range want.Headers {
					if first.Headers[name] != value {
						t.Fatalf("request header %s: %q, want %q", name, first.Headers[name], value)
					}
				}
			}
		})
	}
}
