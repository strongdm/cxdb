// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func testOAuthServer(t *testing.T) (*OAuthServer, *SessionStore) {
	t.Helper()
	store, err := NewSessionStore(filepath.Join(t.TempDir(), "oauth.sqlite"), "session", time.Hour, "", false, "oauth-test-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	server, err := NewOAuthServer(store, "https://cxdb.example", "/auth/login")
	if err != nil {
		t.Fatal(err)
	}
	return server, store
}

func TestOAuthAuthorizationCodePKCEAndSingleUse(t *testing.T) {
	server, store := testOAuthServer(t)
	clientID, redirectURI := registerTestClient(t, server, "http://127.0.0.1:49152/callback")
	sessionID, err := store.CreateForIdentity(context.Background(), "https://issuer.example", "alice", "alice@example.com", "Alice", "", "oidc", []string{"cxdb:read", "cxdb:write"})
	if err != nil {
		t.Fatal(err)
	}
	verifier := strings.Repeat("a", 64)
	digest := sha256.Sum256([]byte(verifier))
	request := oauthAuthorizationRequest{
		ClientID: clientID, RedirectURI: redirectURI, State: "client-state",
		Challenge: base64.RawURLEncoding.EncodeToString(digest[:]), Scopes: []string{"cxdb:read", "cxdb:write"},
		Resource: server.resource, ExpiresAt: time.Now().Add(time.Minute).Unix(),
	}
	signed, err := server.signAuthorizationRequest(request)
	if err != nil {
		t.Fatal(err)
	}
	form := url.Values{"request": {signed}, "decision": {"allow"}}
	authorizeRequest := httptest.NewRequest(http.MethodPost, "/oauth/authorize", strings.NewReader(form.Encode()))
	authorizeRequest.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	setSessionCookie(t, store, sessionID, authorizeRequest)
	authorizeResponse := httptest.NewRecorder()
	server.AuthorizeHandler(authorizeResponse, authorizeRequest)
	if authorizeResponse.Code != http.StatusFound {
		t.Fatalf("authorize status = %d, body=%s", authorizeResponse.Code, authorizeResponse.Body.String())
	}
	redirect, err := url.Parse(authorizeResponse.Header().Get("Location"))
	if err != nil {
		t.Fatal(err)
	}
	if redirect.Query().Get("state") != "client-state" || redirect.Query().Get("iss") != "https://cxdb.example" {
		t.Fatalf("authorization response parameters = %s", redirect.RawQuery)
	}
	code := redirect.Query().Get("code")
	tokenForm := url.Values{"grant_type": {"authorization_code"}, "code": {code}, "client_id": {clientID}, "redirect_uri": {redirectURI}, "code_verifier": {verifier}}
	tokenRequest := httptest.NewRequest(http.MethodPost, "/oauth/token", strings.NewReader(tokenForm.Encode()))
	tokenRequest.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	tokenResponse := httptest.NewRecorder()
	server.TokenHandler(tokenResponse, tokenRequest)
	if tokenResponse.Code != http.StatusOK {
		t.Fatalf("token status = %d, body=%s", tokenResponse.Code, tokenResponse.Body.String())
	}
	var tokenPayload map[string]any
	if err := json.Unmarshal(tokenResponse.Body.Bytes(), &tokenPayload); err != nil {
		t.Fatal(err)
	}
	verified, err := server.Verify(tokenPayload["access_token"].(string))
	if err != nil || !verified.HasScope("cxdb:write") || verified.Subject != "alice" {
		t.Fatalf("verified token = %+v, err=%v", verified, err)
	}

	replayRequest := httptest.NewRequest(http.MethodPost, "/oauth/token", strings.NewReader(tokenForm.Encode()))
	replayRequest.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	replayResponse := httptest.NewRecorder()
	server.TokenHandler(replayResponse, replayRequest)
	if replayResponse.Code != http.StatusBadRequest || !strings.Contains(replayResponse.Body.String(), "invalid_grant") {
		t.Fatalf("code replay accepted: status=%d body=%s", replayResponse.Code, replayResponse.Body.String())
	}
	if _, err := server.Verify(tokenPayload["access_token"].(string)); err == nil {
		t.Fatal("authorization-code replay did not revoke the issued access token")
	}
}

func TestOAuthRegistrationRejectsUnsafeRedirects(t *testing.T) {
	server, _ := testOAuthServer(t)
	for _, redirect := range []string{"http://example.com/callback", "https://user@example.com/callback", "https://example.com/callback#fragment", "file:///tmp/callback"} {
		body := `{"redirect_uris":[` + strconvQuote(redirect) + `]}`
		request := httptest.NewRequest(http.MethodPost, "/oauth/register", strings.NewReader(body))
		response := httptest.NewRecorder()
		server.RegisterHandler(response, request)
		if response.Code != http.StatusBadRequest {
			t.Errorf("redirect %q status = %d", redirect, response.Code)
		}
	}
}

func TestOAuthConsentCSPAllowsOnlyRegisteredCallbackOrigin(t *testing.T) {
	server, store := testOAuthServer(t)
	clientID, redirectURI := registerTestClient(t, server, "http://127.0.0.1:49152/callback")
	sessionID, err := store.CreateForIdentity(context.Background(), "https://issuer.example", "alice", "alice@example.com", "Alice", "", "oidc", []string{"cxdb:read", "cxdb:write"})
	if err != nil {
		t.Fatal(err)
	}
	challenge := base64.RawURLEncoding.EncodeToString(make([]byte, sha256.Size))
	query := url.Values{
		"response_type": {"code"}, "client_id": {clientID}, "redirect_uri": {redirectURI},
		"state": {"state"}, "scope": {"cxdb:read"}, "resource": {server.resource},
		"code_challenge": {challenge}, "code_challenge_method": {"S256"},
	}
	request := httptest.NewRequest(http.MethodGet, "/oauth/authorize?"+query.Encode(), nil)
	setSessionCookie(t, store, sessionID, request)
	response := httptest.NewRecorder()
	server.AuthorizeHandler(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("consent status = %d, body=%s", response.Code, response.Body.String())
	}
	want := "default-src 'none'; form-action 'self' http://127.0.0.1:49152; frame-ancestors 'none'; base-uri 'none'"
	if got := response.Header().Get("Content-Security-Policy"); got != want {
		t.Fatalf("consent CSP = %q, want %q", got, want)
	}
}

func TestOAuthConsentCSPCallbackOrigins(t *testing.T) {
	tests := map[string]string{
		"https://client.example/callback?source=cxdb": "https://client.example",
		"http://localhost:6276/oauth/callback":        "http://localhost:6276",
		"http://127.0.0.1:6276/oauth/callback":        "http://127.0.0.1:6276",
		"http://[::1]:6276/oauth/callback":            "http://[::1]:6276",
	}
	for redirectURI, origin := range tests {
		t.Run(origin, func(t *testing.T) {
			csp, err := oauthConsentCSP(redirectURI)
			if err != nil {
				t.Fatal(err)
			}
			want := "default-src 'none'; form-action 'self' " + origin + "; frame-ancestors 'none'; base-uri 'none'"
			if csp != want {
				t.Fatalf("consent CSP = %q, want %q", csp, want)
			}
		})
	}
}

func TestOAuthAuthorizationRequiresState(t *testing.T) {
	server, _ := testOAuthServer(t)
	query := url.Values{
		"response_type": {"code"}, "client_id": {"client"},
		"redirect_uri":   {"http://127.0.0.1:49152/callback"},
		"code_challenge": {"challenge"}, "code_challenge_method": {"S256"},
	}
	if _, err := server.parseAuthorizationRequest(query); err == nil {
		t.Fatal("authorization request without state was accepted")
	}
}

func TestOAuthRegistrationRejectsClientsAtPersistentCap(t *testing.T) {
	server, _ := testOAuthServer(t)
	for i := 0; i < maxOAuthClients; i++ {
		if _, err := server.store.db.Exec(`
			INSERT INTO oauth_clients (client_id, client_name, redirect_uris_json, created_at)
			VALUES (?, ?, ?, ?)
		`, fmt.Sprintf("cap-%d", i), "cap test", `["http://127.0.0.1/callback"]`, time.Now().UTC()); err != nil {
			t.Fatalf("fill client cap at %d: %v", i, err)
		}
	}
	request := httptest.NewRequest(http.MethodPost, "/oauth/register", strings.NewReader(`{"redirect_uris":["http://127.0.0.1/callback"]}`))
	response := httptest.NewRecorder()
	server.RegisterHandler(response, request)
	if response.Code != http.StatusTooManyRequests || !strings.Contains(response.Body.String(), "registration_limit_reached") {
		t.Fatalf("registration at cap status=%d body=%s", response.Code, response.Body.String())
	}
}

func registerTestClient(t *testing.T, server *OAuthServer, redirect string) (string, string) {
	t.Helper()
	body, _ := json.Marshal(map[string]any{"client_name": "test", "redirect_uris": []string{redirect}, "token_endpoint_auth_method": "none"})
	request := httptest.NewRequest(http.MethodPost, "/oauth/register", strings.NewReader(string(body)))
	response := httptest.NewRecorder()
	server.RegisterHandler(response, request)
	if response.Code != http.StatusCreated {
		t.Fatalf("register status = %d, body=%s", response.Code, response.Body.String())
	}
	var payload struct {
		ClientID string `json:"client_id"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &payload); err != nil {
		t.Fatal(err)
	}
	return payload.ClientID, redirect
}

func setSessionCookie(t *testing.T, store *SessionStore, sessionID string, request *http.Request) {
	t.Helper()
	recorder := httptest.NewRecorder()
	store.SetCookie(recorder, sessionID)
	request.AddCookie(recorder.Result().Cookies()[0])
}

func strconvQuote(value string) string {
	raw, _ := json.Marshal(value)
	return string(raw)
}
