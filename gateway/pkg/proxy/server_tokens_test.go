// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"bytes"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"testing/fstest"
	"time"

	"github.com/strongdm/cxdb/gateway/internal/config"
	"github.com/strongdm/cxdb/gateway/pkg/auth"
)

func TestPersonalTokenHTTPCreateListRevokeAndCSRF(t *testing.T) {
	store, err := auth.NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", false, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	sessionID, err := store.CreateForIdentity(t.Context(), "https://issuer.example", "alice", "alice@example.com", "Alice", "", "oidc", []string{"cxdb:read", "cxdb:write"})
	if err != nil {
		t.Fatal(err)
	}
	session, err := store.Get(t.Context(), sessionID)
	if err != nil {
		t.Fatal(err)
	}
	server := &Server{sessions: store}
	cookieRecorder := httptest.NewRecorder()
	store.SetCookie(cookieRecorder, sessionID)
	cookie := cookieRecorder.Result().Cookies()[0]

	badRequest := httptest.NewRequest(http.MethodPost, "/api/v1/tokens", strings.NewReader(`{"name":"laptop","scopes":["cxdb:read"]}`))
	badRequest.AddCookie(cookie)
	badResponse := httptest.NewRecorder()
	server.tokens(badResponse, badRequest)
	if badResponse.Code != http.StatusForbidden {
		t.Fatalf("missing CSRF status = %d", badResponse.Code)
	}

	createRequest := httptest.NewRequest(http.MethodPost, "/api/v1/tokens", strings.NewReader(`{"name":"laptop","scopes":["cxdb:read","cxdb:write"]}`))
	createRequest.AddCookie(cookie)
	createRequest.Header.Set("X-CSRF-Token", store.CSRFToken(session))
	createResponse := httptest.NewRecorder()
	server.tokens(createResponse, createRequest)
	if createResponse.Code != http.StatusCreated {
		t.Fatalf("create status = %d, body=%s", createResponse.Code, createResponse.Body.String())
	}
	var created struct {
		Token     auth.APIToken `json:"token"`
		Plaintext string        `json:"plaintext"`
	}
	if err := json.Unmarshal(createResponse.Body.Bytes(), &created); err != nil {
		t.Fatal(err)
	}
	if created.Plaintext == "" {
		t.Fatal("create did not return the one-time plaintext")
	}

	listRequest := httptest.NewRequest(http.MethodGet, "/api/v1/tokens", nil)
	listRequest.AddCookie(cookie)
	listResponse := httptest.NewRecorder()
	server.tokens(listResponse, listRequest)
	if listResponse.Code != http.StatusOK || bytes.Contains(listResponse.Body.Bytes(), []byte(created.Plaintext)) || bytes.Contains(listResponse.Body.Bytes(), []byte("token_hash")) {
		t.Fatalf("unsafe list response: status=%d body=%s", listResponse.Code, listResponse.Body.String())
	}

	bearerRequest := httptest.NewRequest(http.MethodGet, "/api/v1/tokens", nil)
	bearerRequest.AddCookie(cookie)
	bearerRequest.Header.Set("Authorization", "Bearer "+created.Plaintext)
	bearerResponse := httptest.NewRecorder()
	server.tokens(bearerResponse, bearerRequest)
	if bearerResponse.Code != http.StatusForbidden {
		t.Fatalf("bearer token management status = %d", bearerResponse.Code)
	}

	revokeRequest := httptest.NewRequest(http.MethodDelete, "/api/v1/tokens/"+created.Token.ID, nil)
	revokeRequest.AddCookie(cookie)
	revokeRequest.Header.Set("X-CSRF-Token", store.CSRFToken(session))
	revokeResponse := httptest.NewRecorder()
	server.tokenByID(revokeResponse, revokeRequest)
	if revokeResponse.Code != http.StatusNoContent {
		t.Fatalf("revoke status = %d", revokeResponse.Code)
	}
	if _, err := store.VerifyAPIToken(t.Context(), created.Plaintext); err == nil {
		t.Fatal("revoked token still verifies")
	}
}

func TestProductionHandlerPersonalTokenLifecycleAndAPIUse(t *testing.T) {
	store, err := auth.NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", false, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"ok":true}`)
	}))
	t.Cleanup(backend.Close)
	reverse, err := NewReverseProxy(backend.URL, slog.Default())
	if err != nil {
		t.Fatal(err)
	}
	cfg := config.Config{
		PublicBaseURL: "http://localhost:8080", CXDBBackendURL: backend.URL,
		Port: "0", DevMode: true,
	}
	server, err := New(cfg, store, nil, reverse, fstest.MapFS{
		"index.html": {Data: []byte("<!doctype html><title>CXDB</title>")},
	}, slog.Default())
	if err != nil {
		t.Fatal(err)
	}
	remote := httptest.NewServer(server.Handler())
	t.Cleanup(remote.Close)

	meResponse, err := http.Get(remote.URL + "/api/v1/me")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = meResponse.Body.Close() }()
	var me struct {
		CSRFToken string `json:"csrf_token"`
	}
	if meResponse.StatusCode != http.StatusOK || json.NewDecoder(meResponse.Body).Decode(&me) != nil || me.CSRFToken == "" {
		t.Fatalf("me response status = %d", meResponse.StatusCode)
	}

	createRequest, _ := http.NewRequest(http.MethodPost, remote.URL+"/api/v1/tokens", strings.NewReader(`{"name":"integration","scopes":["cxdb:read","cxdb:write"]}`))
	createRequest.Header.Set("Content-Type", "application/json")
	createRequest.Header.Set("X-CSRF-Token", me.CSRFToken)
	createResponse, err := http.DefaultClient.Do(createRequest)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = createResponse.Body.Close() }()
	var created struct {
		Token     auth.APIToken `json:"token"`
		Plaintext string        `json:"plaintext"`
	}
	if createResponse.StatusCode != http.StatusCreated || json.NewDecoder(createResponse.Body).Decode(&created) != nil || created.Plaintext == "" {
		t.Fatalf("create response status = %d", createResponse.StatusCode)
	}

	apiRequest, _ := http.NewRequest(http.MethodGet, remote.URL+"/v1/private", nil)
	apiRequest.Header.Set("Authorization", "Bearer "+created.Plaintext)
	apiResponse, err := http.DefaultClient.Do(apiRequest)
	if err != nil {
		t.Fatal(err)
	}
	if err := apiResponse.Body.Close(); err != nil {
		t.Fatal(err)
	}
	if apiResponse.StatusCode != http.StatusOK {
		t.Fatalf("API token request status = %d", apiResponse.StatusCode)
	}

	revokeRequest, _ := http.NewRequest(http.MethodDelete, remote.URL+"/api/v1/tokens/"+created.Token.ID, nil)
	revokeRequest.Header.Set("X-CSRF-Token", me.CSRFToken)
	revokeResponse, err := http.DefaultClient.Do(revokeRequest)
	if err != nil {
		t.Fatal(err)
	}
	if err := revokeResponse.Body.Close(); err != nil {
		t.Fatal(err)
	}
	if revokeResponse.StatusCode != http.StatusNoContent {
		t.Fatalf("revoke response status = %d", revokeResponse.StatusCode)
	}

	revokedRequest, _ := http.NewRequest(http.MethodGet, remote.URL+"/v1/private", nil)
	revokedRequest.Header.Set("Authorization", "Bearer "+created.Plaintext)
	revokedResponse, err := http.DefaultClient.Do(revokedRequest)
	if err != nil {
		t.Fatal(err)
	}
	if err := revokedResponse.Body.Close(); err != nil {
		t.Fatal(err)
	}
	if revokedResponse.StatusCode != http.StatusUnauthorized {
		t.Fatalf("revoked token status = %d", revokedResponse.StatusCode)
	}
}
