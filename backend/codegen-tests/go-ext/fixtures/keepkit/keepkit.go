// Package keepkit is a stand-in for a third-party library whose exported
// type name differs from its Rust sibling on purpose: the same logical
// contract is Vault<T> there and Store[T] here. Concrete type, inherent
// methods, so the per-language instantiation name is the whole story.
package keepkit

import "context"

type Store[T any] struct {
	seed T
}

func OpenStore[T any](seed T) (*Store[T], error) {
	return &Store[T]{seed: seed}, nil
}

func (s *Store[T]) Get(ctx context.Context) (T, error) {
	return s.seed, nil
}
