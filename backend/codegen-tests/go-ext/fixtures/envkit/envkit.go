// Package envkit is a stand-in for a third-party settings-provider library:
// a provider constructed once, whose methods resolve the endpoints several
// operations of the generated SDK read. The repro for a field sourced from
// a foreign handle's method.
package envkit

import "context"

type Config struct {
	ReadURL  string
	WriteURL string
}

type Provider struct {
	name string
}

func NewProvider(name string) (*Provider, error) {
	return &Provider{name: name}, nil
}

func (p *Provider) Get(ctx context.Context) (Config, error) {
	return Config{ReadURL: "https://read." + p.name, WriteURL: "https://write." + p.name}, nil
}

func (p *Provider) GetFor(region string) (Config, error) {
	return Config{ReadURL: "https://" + region + ".read." + p.name, WriteURL: "https://" + region + ".write." + p.name}, nil
}
