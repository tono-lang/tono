// Package envkit is a stand-in for a real third-party settings-provider
// library: the recipe binds against it declaratively (no bespoke code), so
// this file only exists to give the generated SDK something real to compile
// against.
package envkit

import "context"

type Endpoints struct {
	ReadURL  string
	WriteURL string
}

type Provider struct {
	name string
}

func NewEnvironmentProvider(name string) (*Provider, error) {
	return &Provider{name: name}, nil
}

func (p *Provider) Get(ctx context.Context) (Endpoints, error) {
	return Endpoints{ReadURL: "https://read." + p.name, WriteURL: "https://write." + p.name}, nil
}
