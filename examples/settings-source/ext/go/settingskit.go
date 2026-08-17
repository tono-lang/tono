// Package settingskit is a stand-in for a real third-party settings source
// library that is generic over the value it resolves: the recipe binds
// against it declaratively (no bespoke code), so this file only exists to
// give the generated SDK something real, generic, to compile against.
package settingskit

import "context"

type Source[T any] struct {
	value T
}

func (s *Source[T]) Get(ctx context.Context) (T, error) {
	return s.value, nil
}

func NewEnvSource[T any](service, region string) (*Source[T], error) {
	var value T
	return &Source[T]{value: value}, nil
}
