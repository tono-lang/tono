// Package configlib is a stand-in for a real third-party config library:
// the recipe binds against it declaratively (no bespoke code), so this file
// only exists to give the generated SDK something real to compile against.
package configlib

type Config struct {
	Host    string
	DevHost string
	Env     string
	Token   string
}

func Load(service, region string) (Config, error) {
	return Config{
		Host:    service + "." + region + ".prod.internal",
		DevHost: service + "." + region + ".dev.internal",
		Env:     "dev",
		Token:   "s3cr3t",
	}, nil
}
