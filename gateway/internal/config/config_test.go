// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package config

import (
	"strings"
	"testing"
)

func TestValidateRequiresStrongSessionSecret(t *testing.T) {
	cfg := Config{SessionSecret: "short", CXDBBackendURL: "http://127.0.0.1:9010"}
	if err := cfg.validate(); err == nil {
		t.Fatal("short session secret was accepted")
	}
}

func TestValidateAcceptsStrongSessionSecretWithoutGoogleWhenDevMode(t *testing.T) {
	cfg := Config{
		SessionSecret:  "01234567890123456789012345678901",
		CXDBBackendURL: "http://127.0.0.1:9010",
		DevMode:        true,
	}
	if err := cfg.validate(); err != nil {
		t.Fatalf("strong development configuration rejected: %v", err)
	}
}

func TestValidateRejectsInvalidTrustedProxyCIDR(t *testing.T) {
	cfg := Config{
		SessionSecret:     strings.Repeat("s", 32),
		CXDBBackendURL:    "http://127.0.0.1:9010",
		DevMode:           true,
		TrustedProxyCIDRs: []string{"not-a-cidr"},
	}
	if err := cfg.validate(); err == nil || !strings.Contains(err.Error(), "TRUSTED_PROXY_CIDRS") {
		t.Fatalf("validate() error = %v", err)
	}
}

func TestIsLocalhostURLRequiresExactHost(t *testing.T) {
	tests := map[string]bool{
		"http://localhost:8080":             true,
		"https://127.0.0.1:8080":            true,
		"http://[::1]:8080":                 true,
		"http://LOCALHOST/":                 true,
		"http://localhost.attacker.example": false,
		"http://127.0.0.1.attacker.example": false,
		"http://user@localhost:8080":        false,
		"localhost:8080":                    false,
	}
	for raw, want := range tests {
		if got := isLocalhostURL(raw); got != want {
			t.Errorf("isLocalhostURL(%q) = %v, want %v", raw, got, want)
		}
	}
}
