// Package auth handles bearer tokens.
package auth

import (
	"errors"
	"strings"

	"example.com/shop/validation"
)

// Token is a decoded bearer token.
type Token struct {
	User    string
	Scope   string
	Expired bool
}

// Parse splits a bearer token into its parts and checks them.
func Parse(raw string) (Token, error) {
	parts := strings.Split(raw, ".")
	if !validation.Validate(parts) {
		return Token{}, errors.New("bad token")
	}
	return decode(parts), nil
}

func decode(parts []string) Token {
	return Token{User: parts[0], Scope: parts[1]}
}

// Refresh renews an expired token, giving up after three attempts.
func Refresh(token Token, attempts int) *Token {
	if attempts > 3 {
		return nil
	}
	if token.Expired {
		token.Expired = false
		return Refresh(token, attempts+1)
	}
	return &token
}
