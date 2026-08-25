// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import "testing"

func TestRequireHTTPSURL(t *testing.T) {
	t.Parallel()

	for _, raw := range []string{
		"http://issuer.example",
		"https://user@issuer.example",
		"//issuer.example",
		"https:///missing-host",
	} {
		if err := requireHTTPSURL(raw, "test URL"); err == nil {
			t.Errorf("requireHTTPSURL(%q) succeeded", raw)
		}
	}
	if err := requireHTTPSURL("https://issuer.example/path", "test URL"); err != nil {
		t.Fatalf("valid HTTPS URL failed: %v", err)
	}
}
