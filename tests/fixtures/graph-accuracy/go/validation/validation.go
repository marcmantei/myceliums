// Package validation checks input.
package validation

import "strings"

// Rule is one check.
type Rule func(string) bool

// Validate checks the parts of a token.
func Validate(parts []string) bool {
	return len(parts) == 2 && parts[0] != "" && parts[1] != ""
}

// IsEmail reports whether value looks like an address.
func IsEmail(value string) bool {
	return strings.Contains(value, "@")
}

// Validator applies rules.
type Validator struct {
	rules []Rule
}

// NewValidator returns a Validator for rules.
func NewValidator(rules ...Rule) *Validator {
	return &Validator{rules: rules}
}

// Validate applies every rule to value.
func (v *Validator) Validate(value string) bool {
	for _, rule := range v.rules {
		if !v.check(rule, value) {
			return false
		}
	}
	return true
}

func (v *Validator) check(rule Rule, value string) bool {
	return rule(value)
}
