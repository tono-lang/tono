// Package companybus is a stand-in for the third-party bus library the
// generated SDK integrates with.
package companybus

import "errors"

var ErrBusy = errors.New("bus overloaded")

type Ack struct {
	ID string
	OK bool
}

type Publisher struct {
	endpoint, token string
}

func Connect(endpoint, token string) (*Publisher, error) {
	return &Publisher{endpoint: endpoint, token: token}, nil
}

func (p *Publisher) Send(topic, body string) (Ack, error) {
	return Ack{ID: "n1", OK: true}, nil
}
