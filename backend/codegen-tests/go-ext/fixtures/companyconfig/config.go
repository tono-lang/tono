// Package companyconfig is a stand-in for the third-party config library the
// generated SDK integrates with.
package companyconfig

type Credentials struct {
	Secret string
}

type Config struct {
	Host        string
	DevHost     string
	Env         string
	Credentials Credentials
}

func Load(service, region string) (Config, error) {
	return Config{
		Host:        "prod.internal",
		DevHost:     "dev.internal",
		Env:         "dev",
		Credentials: Credentials{Secret: "s3cr3t"},
	}, nil
}
