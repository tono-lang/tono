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
	Name       string          `json:"name"`
	Descriptor json.RawMessage `json:"descriptor"`
	Values     map[string]any  `json:"values"`
	Input      map[string]any  `json:"input"`
	Script     []parityStep    `json:"script"`
	Expect     parityExpect    `json:"expect"`
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

func scriptedTransport(t *testing.T, script []parityStep, attempts *int) Transport {
	return func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
		if *attempts >= len(script) {
			t.Fatalf("transport called %d times for a %d-step script", *attempts+1, len(script))
		}
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
			d, err := ParseDescriptor(vector.Descriptor)
			if err != nil {
				t.Fatalf("descriptor: %v", err)
			}
			attempts := 0
			r, delays := deterministic(t, Options{
				BaseURL:   "https://api.test",
				Transport: scriptedTransport(t, vector.Script, &attempts),
				Values:    vector.Values,
			})
			outcome, err := r.Execute(context.Background(), d, vector.Input, nil)
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
		})
	}
}
