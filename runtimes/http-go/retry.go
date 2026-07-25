package tonohttp

import (
	"encoding/json"
	"math"
	"time"
)

// Backoff is a fixed runtime policy (exponential with full jitter), not
// declarable in the descriptor: delay before retry n is
// random() * min(cap, base * 2^n). The constants are part of the cross-runtime
// parity contract and must match the other HTTP runtimes.
const (
	backoffBaseMs = 100
	backoffCapMs  = 2000
)

func backoffDelay(retry int, random float64) time.Duration {
	exp := math.Min(backoffCapMs, backoffBaseMs*math.Pow(2, float64(retry)))
	return time.Duration(random * exp * float64(time.Millisecond))
}

// resolveNumber reads a ValueSource: a literal yields itself; a ref yields the
// numeric value under that name in Options.Values. Anything else (absent ref,
// non-numeric value) yields false, which disables the feature rather than
// failing the call: the descriptor and the resolved values are produced by
// generated code, and the runtime stays blind to their provenance.
func resolveNumber(s ValueSource, values map[string]any) (float64, bool) {
	if s.Lit != nil {
		return *s.Lit, true
	}
	if s.Ref == nil {
		return 0, false
	}
	switch v := values[*s.Ref].(type) {
	case float64:
		return v, true
	case int:
		return float64(v), true
	case int64:
		return float64(v), true
	}
	return 0, false
}

// resolveMaxRetries yields the maximum number of retries after the first
// attempt. No retry declaration, a non-numeric value, or a value below one all
// mean zero retries; a fractional value floors.
func resolveMaxRetries(spec *RetrySpec, values map[string]any) int {
	if spec == nil {
		return 0
	}
	n, ok := resolveNumber(spec.Max, values)
	if !ok || n < 1 {
		return 0
	}
	return int(math.Floor(n))
}

// resolveTimeout yields the per-attempt deadline. Absent or non-numeric means
// no deadline; a non-positive value flows through and is treated as no
// deadline by the single `timeout > 0` check at the point of use.
func resolveTimeout(source *ValueSource, values map[string]any) time.Duration {
	if source == nil {
		return 0
	}
	ms, ok := resolveNumber(*source, values)
	if !ok {
		return 0
	}
	return time.Duration(ms * float64(time.Millisecond))
}

// bodyCode reads the "code" discriminator field from an error body, when the
// body is a JSON object carrying one as a string.
func bodyCode(body string) *string {
	var object map[string]any
	if err := json.Unmarshal([]byte(body), &object); err != nil {
		return nil
	}
	if code, ok := object["code"].(string); ok {
		return &code
	}
	return nil
}

// isRetryable classifies one outcome for the retry loop: a transport failure
// always retries; an error status retries only when it matches a declared
// retryable error. Matching walks the declared errors in descriptor order and
// the first status match with an agreeing code decides (a declared code must
// equal the body's "code" field; a null code matches any body).
func isRetryable(d *WireDescriptor, o Outcome) bool {
	if o.Kind == OutcomeTransport {
		return true
	}
	if o.Kind != OutcomeError {
		return false
	}
	code := bodyCode(o.Body)
	for _, e := range d.Errors {
		if e.Status != o.Status {
			continue
		}
		if e.Code == nil || (code != nil && *code == *e.Code) {
			return e.Retryable
		}
	}
	return false
}
