// Package compose is a stand-in for a third-party library that composes its
// own handles: NewCombined receives two already-constructed Resource values
// back as arguments, a shape a wrapped SDK vendor can actually declare.
package compose

type Value struct {
	Data string
}

// Resource is the real exported type every constructor below returns and
// NewCombined itself also accepts: a generated SDK must pass this type raw
// between two of the library's own calls, never tono's own adapter.
type Resource struct {
	get func() (Value, error)
}

func (r *Resource) Get() (Value, error) {
	return r.get()
}

func NewPrimary(a, b, c, d string) (*Resource, error) {
	return &Resource{
		get: func() (Value, error) {
			return Value{Data: "primary:" + a + "/" + b + "/" + c + "/" + d}, nil
		},
	}, nil
}

func NewSecondary(b, c string) (*Resource, error) {
	return &Resource{
		get: func() (Value, error) {
			return Value{Data: "secondary:" + b + "/" + c}, nil
		},
	}, nil
}

func NewCombined(b string, primary, secondary *Resource) (*Resource, error) {
	return &Resource{
		get: func() (Value, error) {
			if v, err := primary.Get(); err == nil {
				return v, nil
			}
			return secondary.Get()
		},
	}, nil
}
