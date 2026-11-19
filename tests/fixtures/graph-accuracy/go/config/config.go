// Package config loads settings.
package config

import (
	"encoding/json"
	"strings"
)

// Parse reads a JSON configuration document.
func Parse(text string) (map[string]string, error) {
	var data map[string]string
	if err := json.Unmarshal([]byte(text), &data); err != nil {
		return nil, err
	}
	return normalize(data), nil
}

func normalize(data map[string]string) map[string]string {
	out := make(map[string]string, len(data))
	for key, value := range data {
		out[strings.ToLower(key)] = value
	}
	return out
}

// Config holds settings.
type Config struct {
	values map[string]string
}

// New wraps values in a Config.
func New(values map[string]string) *Config {
	return &Config{values: values}
}

// Load parses text into a Config.
func Load(text string) (*Config, error) {
	values, err := Parse(text)
	if err != nil {
		return nil, err
	}
	return New(values), nil
}

// Get returns the value for key, or fallback.
func (c *Config) Get(key, fallback string) string {
	if value, ok := c.values[key]; ok {
		return value
	}
	return fallback
}
