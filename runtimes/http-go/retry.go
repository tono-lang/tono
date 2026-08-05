package tonohttp

import (
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

// isRetryable classifies one outcome for the retry loop: a transport failure
// always retries; a success never does; an error status retries only when the
// caller-supplied predicate accepts its status and raw body. The predicate is
// the generated client's own decode/Retryable() pair, so a nil predicate (an
// op with no declared errors) means no error response is ever retryable.
func isRetryable(o Outcome, retryable func(status int, body string) bool) bool {
	if o.Kind == OutcomeTransport {
		return true
	}
	if o.Kind != OutcomeError || retryable == nil {
		return false
	}
	return retryable(o.Status, o.Body)
}
