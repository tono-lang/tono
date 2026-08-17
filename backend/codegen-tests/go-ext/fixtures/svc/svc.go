// Package svc is a stand-in for a third-party library whose own method
// call takes a context as its first parameter (Go's own idiom): the repro
// for the "extern ctx" Go emission.
package svc

import "context"

type Conn struct{}

func Connect() (*Conn, error) {
	return &Conn{}, nil
}

func (c *Conn) Get(ctx context.Context, id string) (string, error) {
	return id, nil
}
