// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestSecurityHeadersPreservesHandlerSpecificCSP(t *testing.T) {
	server := &Server{cspHeader: "default-src 'self'; form-action 'self'"}
	consentCSP := "default-src 'none'; form-action 'self' http://127.0.0.1:6276"
	handler := server.securityHeaders(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Security-Policy", consentCSP)
		w.WriteHeader(http.StatusOK)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/oauth/authorize", nil))
	if got := response.Header().Get("Content-Security-Policy"); got != consentCSP {
		t.Fatalf("Content-Security-Policy = %q, want %q", got, consentCSP)
	}
}
