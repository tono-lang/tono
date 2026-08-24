// Package mathkit is a stand-in for a third-party numeric library with the
// shapes real Go libraries have, kept deliberately (this is the FFI bench,
// not a fixture bent to fit the emitter): the handle is an interface, not a
// pointer to a struct; the constructors are generic over the value they
// produce; the formula constructor takes functional variadic options; and
// the fallback constructor composes handles it already built.
package mathkit

import (
	"context"
	"errors"
	"fmt"
	"math"
	"strconv"
	"strings"
)

// Calculator produces one value. Every constructor below returns this
// interface, never a concrete type, so a generated SDK must hold and pass
// the interface value itself.
type Calculator[T any] interface {
	Compute(ctx context.Context) (T, error)
	Close() error
}

// Option configures FromFormula. Options are functional and variadic, the
// idiomatic Go shape; there is no options struct.
type Option func(*settings)

type settings struct {
	precision int
}

// WithPrecision rounds the formula result to the given number of digits.
func WithPrecision(digits int) Option {
	return func(s *settings) { s.precision = digits }
}

// ErrNoStrategy is returned when FromFallback receives a strategy it does
// not know.
var ErrNoStrategy = errors.New("mathkit: unknown fallback strategy")

// ErrParse is wrapped by every formula that does not parse, so a caller
// recognises the failure with errors.Is.
var ErrParse = errors.New("mathkit: cannot parse expression")

type constant[T any] struct{ value T }

func (c *constant[T]) Compute(ctx context.Context) (T, error) { return c.value, nil }
func (c *constant[T]) Close() error                           { return nil }

// FromConstant answers the same value on every Compute.
func FromConstant[T any](value T) (Calculator[T], error) {
	return &constant[T]{value: value}, nil
}

type formula[T any] struct {
	expr      string
	precision int
}

// Compute evaluates "<a> <op> <b>" for float64 results (the only
// instantiation the bench exercises); any other T fails loudly.
func (f *formula[T]) Compute(ctx context.Context) (T, error) {
	var zero T
	fields := strings.Fields(f.expr)
	if len(fields) != 3 {
		return zero, fmt.Errorf("%w: %q", ErrParse, f.expr)
	}
	a, err := strconv.ParseFloat(fields[0], 64)
	if err != nil {
		return zero, err
	}
	b, err := strconv.ParseFloat(fields[2], 64)
	if err != nil {
		return zero, err
	}
	var out float64
	switch fields[1] {
	case "+":
		out = a + b
	case "-":
		out = a - b
	case "*":
		out = a * b
	case "/":
		out = a / b
	default:
		return zero, fmt.Errorf("mathkit: unknown operator %q", fields[1])
	}
	if f.precision > 0 {
		scale := math.Pow(10, float64(f.precision))
		out = math.Round(out*scale) / scale
	}
	v, ok := any(out).(T)
	if !ok {
		return zero, fmt.Errorf("mathkit: formula results are float64, not %T", zero)
	}
	return v, nil
}
func (f *formula[T]) Close() error { return nil }

// FromFormula evaluates an expression, shaped by zero or more options.
func FromFormula[T any](expr string, opts ...Option) (Calculator[T], error) {
	s := settings{}
	for _, o := range opts {
		o(&s)
	}
	return &formula[T]{expr: expr, precision: s.precision}, nil
}

type series[T any] struct{ values []T }

// Compute answers the last value of the series.
func (s *series[T]) Compute(ctx context.Context) (T, error) {
	var zero T
	if len(s.values) == 0 {
		return zero, errors.New("mathkit: empty series")
	}
	return s.values[len(s.values)-1], nil
}
func (s *series[T]) Close() error { return nil }

// FromSeries answers from a collection of values.
func FromSeries[T any](values []T) (Calculator[T], error) {
	return &series[T]{values: values}, nil
}

type fallback[T any] struct {
	strategy string
	calcs    []Calculator[T]
}

// Compute asks each composed calculator in turn ("first" answers the first
// success, "last" the last one).
func (f *fallback[T]) Compute(ctx context.Context) (T, error) {
	var zero T
	var last T
	var lastErr error = errors.New("mathkit: no calculators to fall back to")
	for _, c := range f.calcs {
		v, err := c.Compute(ctx)
		if err != nil {
			lastErr = err
			continue
		}
		if f.strategy == "first" {
			return v, nil
		}
		last, lastErr = v, nil
	}
	if lastErr != nil {
		return zero, lastErr
	}
	return last, nil
}

func (f *fallback[T]) Close() error {
	for _, c := range f.calcs {
		if err := c.Close(); err != nil {
			return err
		}
	}
	return nil
}

// FromFallback composes calculators the caller already built: the library
// takes its own handles back as arguments, variadically.
func FromFallback[T any](strategy string, calcs ...Calculator[T]) (Calculator[T], error) {
	if strategy != "first" && strategy != "last" {
		return nil, ErrNoStrategy
	}
	return &fallback[T]{strategy: strategy, calcs: calcs}, nil
}

// Client is a session with a remote calculation service. Its name is the
// one every generated SDK's entry takes as well, on purpose: a spelling
// naming it is the library's, and the generated code must keep the package
// selector on it.
type Client struct{ addr string }

// Open starts a session with the service at addr.
func Open(addr string) (*Client, error) {
	return &Client{addr: addr}, nil
}

// Ping answers the service's greeting.
func (c *Client) Ping() (string, error) {
	return "pong from " + c.addr, nil
}

// Memo keeps one value of the caller's own type: the library is generic
// over a type it never sees, the way a settings or cache library is.
type Memo[T any] struct{ value T }

// Remember keeps value for a later Recall.
func Remember[T any](value T) (*Memo[T], error) {
	return &Memo[T]{value: value}, nil
}

// Recall answers the value Remember kept.
func (m *Memo[T]) Recall(ctx context.Context) (T, error) {
	return m.value, nil
}

// Options configures a session. Connect takes it by pointer, the way a Go
// client library takes its options struct: a generated SDK must pass the
// address of the literal it builds, while the type itself stays Options.
type Options struct{ Addr string }

// Connect starts a session with the service opt names.
func Connect(opt *Options) (*Client, error) {
	if opt == nil {
		return nil, errors.New("mathkit: nil options")
	}
	return &Client{addr: opt.Addr}, nil
}
