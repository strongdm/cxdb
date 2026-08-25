// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"testing"
	"time"
)

func Test_isPublicPath(t *testing.T) {
	t.Parallel()

	cases := []struct {
		name string
		path string
		want bool
	}{
		{name: "healthz", path: "/healthz", want: true},
		{name: "oauth", path: "/auth/google/login", want: true},
		{name: "oidc", path: "/auth/oidc/login", want: true},
		{name: "mcp", path: "/mcp", want: true},
		{name: "oauth_metadata", path: "/.well-known/oauth-authorization-server", want: true},
		{name: "resource_metadata", path: "/.well-known/oauth-protected-resource/mcp", want: true},
		{name: "openapi", path: "/openapi.json", want: true},
		{name: "llms", path: "/llms.txt", want: true},
		{name: "unknown_json", path: "/private.json", want: false},
		{name: "unknown_txt", path: "/private.txt", want: false},
		{name: "unknown_auth", path: "/auth/private", want: false},
		{name: "contexts_list", path: "/v1/contexts", want: false},
		{name: "contexts_search", path: "/v1/contexts/search", want: false},
		{name: "context_detail", path: "/v1/contexts/abc123", want: false},
		{name: "metrics", path: "/v1/metrics", want: false},
		{name: "events", path: "/v1/events", want: false},
		{name: "api_javascript_suffix", path: "/v1/private.js", want: false},
		{name: "unknown_root_asset", path: "/private.js", want: false},
		{name: "next_static_asset", path: "/_next/static/app.js", want: true},
	}

	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()

			if got := isPublicPath(tc.path); got != tc.want {
				t.Fatalf("isPublicPath(%q) = %v, want %v", tc.path, got, tc.want)
			}
		})
	}
}

func Test_isPublicWritePath(t *testing.T) {
	t.Parallel()

	cases := []struct {
		name string
		path string
		want bool
	}{
		{name: "healthz", path: "/healthz", want: true},
		{name: "readyz", path: "/readyz", want: true},
		{name: "auth_endpoint", path: "/auth/aws/token", want: true},
		{name: "contexts_list", path: "/v1/contexts", want: false},
		{name: "metrics", path: "/v1/metrics", want: false},
	}

	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()

			if got := isPublicWritePath(tc.path); got != tc.want {
				t.Fatalf("isPublicWritePath(%q) = %v, want %v", tc.path, got, tc.want)
			}
		})
	}
}

func TestWriteMiddlewareEnforcesAPITokenScope(t *testing.T) {
	store, err := NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", false, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	_, plaintext, err := store.CreateAPIToken(context.Background(), APITokenCreateRequest{Name: "reader", Issuer: "issuer", Subject: "alice", Scopes: []string{"cxdb:read"}})
	if err != nil {
		t.Fatal(err)
	}
	handler := RequireAuthForWrites(AuthMiddlewareOptions{Store: store, TokenVerifiers: []BearerTokenVerifier{NewAPITokenVerifier(store)}}, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	request := httptest.NewRequest(http.MethodPost, "/v1/contexts/create", nil)
	request.Header.Set("Authorization", "Bearer "+plaintext)
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusForbidden {
		t.Fatalf("read-only token write status = %d", response.Code)
	}
}

func TestWriteMiddlewareRejectsAnonymousMutation(t *testing.T) {
	store, err := NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", false, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	handler := RequireAuthForWrites(AuthMiddlewareOptions{Store: store}, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	request := httptest.NewRequest(http.MethodPost, "/v1/contexts/create", nil)
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous write status = %d", response.Code)
	}
}

func TestReadMiddlewareRejectsAnonymousSensitiveReads(t *testing.T) {
	store, err := NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", false, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	handler := RequireAuthForReadsWithOptions(AuthMiddlewareOptions{Store: store}, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		t.Errorf("unauthenticated request reached protected handler")
		w.WriteHeader(http.StatusNoContent)
	}))
	for _, path := range []string{"/v1/contexts", "/v1/metrics", "/v1/events"} {
		t.Run(path, func(t *testing.T) {
			request := httptest.NewRequest(http.MethodGet, path, nil)
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, request)
			if response.Code != http.StatusUnauthorized {
				t.Fatalf("anonymous %s status = %d", path, response.Code)
			}
		})
	}
}

func TestInvalidBearerDoesNotFallBackToCookie(t *testing.T) {
	store, err := NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", false, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	sessionID, err := store.Create(context.Background(), "alice@example.com", "Alice", "")
	if err != nil {
		t.Fatal(err)
	}
	cookieRecorder := httptest.NewRecorder()
	store.SetCookie(cookieRecorder, sessionID)
	handler := RequireAuthForReadsWithOptions(AuthMiddlewareOptions{Store: store, TokenVerifiers: []BearerTokenVerifier{NewAPITokenVerifier(store)}}, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	request := httptest.NewRequest(http.MethodGet, "/v1/contexts/1", nil)
	request.AddCookie(cookieRecorder.Result().Cookies()[0])
	request.Header.Set("Authorization", "Bearer invalid")
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusUnauthorized {
		t.Fatalf("invalid bearer with valid cookie status = %d", response.Code)
	}
}
