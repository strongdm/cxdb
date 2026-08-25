// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestReverseProxyReplacesForwardedAndStripsCredentials(t *testing.T) {
	seen := make(chan http.Header, 1)
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		seen <- r.Header.Clone()
		w.WriteHeader(http.StatusNoContent)
	}))
	defer backend.Close()

	reverse, err := NewReverseProxy(backend.URL, slog.Default())
	if err != nil {
		t.Fatal(err)
	}
	req := httptest.NewRequest(http.MethodPost, "/v1/contexts/1/append", strings.NewReader("{}"))
	req.RemoteAddr = "198.51.100.7:44321"
	req.Host = "cxdb.example"
	req.Header.Set("X-Forwarded-For", "203.0.113.9")
	req.Header.Set("Forwarded", "for=203.0.113.9")
	req.Header.Set("X-Forwarded-Proto", "https")
	req.Header.Set("X-Forwarded-Host", "evil.example")
	req.Header.Set("X-Cxdb-Writer-Method", "admin")
	req.Header.Set("X-Cxdb-Writer-Subject", "admin")
	req.Header.Set("X-Cxdb-Writer-Issuer", "admin")
	req.Header.Set("X-Cxdb-User-Email", "admin@example.com")
	req.Header.Set("X-Cxdb-Unknown", "admin")
	req.Header.Set("Authorization", "Bearer gateway-token")
	req.Header.Set("Cookie", "cxdb_session=browser-cookie")

	response := httptest.NewRecorder()
	reverse.ServeHTTP(response, req)
	if response.Code != http.StatusNoContent {
		t.Fatalf("proxy status = %d", response.Code)
	}
	got := <-seen
	if got.Get("X-Forwarded-For") != "198.51.100.7" {
		t.Fatalf("X-Forwarded-For = %q", got.Get("X-Forwarded-For"))
	}
	if got.Get("Forwarded") != "" {
		t.Fatalf("Forwarded was preserved: %q", got.Get("Forwarded"))
	}
	if got.Get("X-Forwarded-Proto") != "http" {
		t.Fatalf("X-Forwarded-Proto = %q", got.Get("X-Forwarded-Proto"))
	}
	if got.Get("X-Forwarded-Host") != "" {
		t.Fatalf("X-Forwarded-Host was forwarded: %q", got.Get("X-Forwarded-Host"))
	}
	for _, header := range []string{"X-Cxdb-Writer-Method", "X-Cxdb-Writer-Subject", "X-Cxdb-Writer-Issuer", "X-Cxdb-User-Email", "X-Cxdb-Unknown", "Authorization", "Cookie"} {
		if got.Get(header) != "" {
			t.Fatalf("%s was forwarded: %q", header, got.Get(header))
		}
	}
}
