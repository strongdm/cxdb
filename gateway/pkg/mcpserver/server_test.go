// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package mcpserver

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"html"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
	cxdbauth "github.com/strongdm/cxdb/gateway/pkg/auth"
	"github.com/vmihailenco/msgpack/v5"
)

type staticVerifier struct{ scopes []string }

func (v staticVerifier) Verify(token string) (*cxdbauth.Session, error) {
	if token != "test-token" {
		return nil, cxdbauth.ErrAPITokenNotFound
	}
	return &cxdbauth.Session{ID: "test", Issuer: "test", Subject: "user", Email: "user@example.com", Scopes: v.scopes, ExpiresAt: time.Now().Add(time.Hour)}, nil
}

type bearerTransport struct {
	base  http.RoundTripper
	token string
}

func (t bearerTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	clone := request.Clone(request.Context())
	clone.Header = request.Header.Clone()
	clone.Header.Set("Authorization", "Bearer "+t.token)
	return t.base.RoundTrip(clone)
}

func TestOfficialClientHandshakeReadAndWriteTools(t *testing.T) {
	var appended bool
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/v1/contexts":
			_, _ = io.WriteString(w, `{"contexts":[{"context_id":"1"}]}`)
		case r.Method == http.MethodPost && r.URL.Path == "/v1/contexts/1/append":
			var body map[string]any
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Fatal(err)
			}
			if body["type_id"] != "cxdb.ConversationItem" || body["payload_base64"] == "" {
				t.Fatalf("unexpected append body: %#v", body)
			}
			appended = true
			_, _ = io.WriteString(w, `{"turn_id":"2"}`)
		default:
			http.NotFound(w, r)
		}
	}))
	t.Cleanup(backend.Close)

	handler, err := New(backend.URL, "https://cxdb.example/.well-known/oauth-protected-resource/mcp", []cxdbauth.BearerTokenVerifier{staticVerifier{scopes: []string{"cxdb:read", "cxdb:write"}}}, slog.Default())
	if err != nil {
		t.Fatal(err)
	}
	remote := httptest.NewServer(handler)
	t.Cleanup(remote.Close)

	client := mcp.NewClient(&mcp.Implementation{Name: "cxdb-test", Version: "1"}, nil)
	httpClient := &http.Client{Transport: bearerTransport{base: http.DefaultTransport, token: "test-token"}}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	session, err := client.Connect(ctx, &mcp.StreamableClientTransport{Endpoint: remote.URL, HTTPClient: httpClient, DisableStandaloneSSE: true}, nil)
	if err != nil {
		t.Fatalf("official MCP client handshake: %v", err)
	}
	defer func() { _ = session.Close() }()
	if _, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "cxdb_list_contexts", Arguments: map[string]any{"limit": 1}}); err != nil {
		t.Fatalf("read tool: %v", err)
	}
	result, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "cxdb_append_message", Arguments: map[string]any{"context_id": "1", "role": "user", "text": "hello"}})
	if err != nil {
		t.Fatalf("write tool: %v", err)
	}
	if result.IsError || !appended {
		t.Fatalf("write tool did not append: result=%+v appended=%v", result, appended)
	}
}

func TestWriteToolRequiresWriteScope(t *testing.T) {
	backend := httptest.NewServer(http.NotFoundHandler())
	t.Cleanup(backend.Close)
	handler, err := New(backend.URL, "https://cxdb.example/metadata", []cxdbauth.BearerTokenVerifier{staticVerifier{scopes: []string{"cxdb:read"}}}, slog.Default())
	if err != nil {
		t.Fatal(err)
	}
	remote := httptest.NewServer(handler)
	t.Cleanup(remote.Close)
	client := mcp.NewClient(&mcp.Implementation{Name: "cxdb-test", Version: "1"}, nil)
	session, err := client.Connect(context.Background(), &mcp.StreamableClientTransport{Endpoint: remote.URL, HTTPClient: &http.Client{Transport: bearerTransport{base: http.DefaultTransport, token: "test-token"}}, DisableStandaloneSSE: true}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = session.Close() }()
	result, err := session.CallTool(context.Background(), &mcp.CallToolParams{Name: "cxdb_create_context", Arguments: map[string]any{}})
	if err != nil {
		t.Fatal(err)
	}
	if !result.IsError {
		t.Fatal("read-only token was allowed to call a write tool")
	}
}

func TestWriteOnlyTokenCanConnectAndUseWriteTool(t *testing.T) {
	var created bool
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost && r.URL.Path == "/v1/contexts/create" {
			created = true
			w.Header().Set("Content-Type", "application/json")
			_, _ = io.WriteString(w, `{"context_id":"1"}`)
			return
		}
		http.NotFound(w, r)
	}))
	t.Cleanup(backend.Close)
	handler, err := New(backend.URL, "https://cxdb.example/metadata", []cxdbauth.BearerTokenVerifier{staticVerifier{scopes: []string{"cxdb:write"}}}, slog.Default())
	if err != nil {
		t.Fatal(err)
	}
	remote := httptest.NewServer(handler)
	t.Cleanup(remote.Close)
	client := mcp.NewClient(&mcp.Implementation{Name: "cxdb-test", Version: "1"}, nil)
	session, err := client.Connect(context.Background(), &mcp.StreamableClientTransport{Endpoint: remote.URL, HTTPClient: &http.Client{Transport: bearerTransport{base: http.DefaultTransport, token: "test-token"}}, DisableStandaloneSSE: true}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = session.Close() }()
	result, err := session.CallTool(context.Background(), &mcp.CallToolParams{Name: "cxdb_create_context", Arguments: map[string]any{}})
	if err != nil {
		t.Fatal(err)
	}
	if result.IsError || !created {
		t.Fatalf("write-only token did not create context: result=%+v created=%v", result, created)
	}
}

func TestCanonicalMessagesUseNumericTags(t *testing.T) {
	tests := []struct {
		role       string
		variantTag int
		textTag    int
	}{
		{role: "user", variantTag: 10, textTag: 1},
		{role: "assistant", variantTag: 11, textTag: 1},
		{role: "system", variantTag: 12, textTag: 3},
	}
	for _, test := range tests {
		t.Run(test.role, func(t *testing.T) {
			payload, err := canonicalMessage(test.role, "hello")
			if err != nil {
				t.Fatal(err)
			}
			var item map[int]msgpack.RawMessage
			if err := msgpack.Unmarshal(payload, &item); err != nil {
				t.Fatal(err)
			}
			var variant map[int]string
			if err := msgpack.Unmarshal(item[test.variantTag], &variant); err != nil {
				t.Fatalf("decode variant tag %d: %v", test.variantTag, err)
			}
			if got := variant[test.textTag]; got != "hello" {
				t.Fatalf("text tag %d = %#v", test.textTag, got)
			}
			if _, exists := item[0]; exists {
				t.Fatal("unexpected zero tag")
			}
		})
	}
}

func TestOAuthAccessTokenConnectsWithOfficialClient(t *testing.T) {
	store, err := cxdbauth.NewSessionStore(filepath.Join(t.TempDir(), "oauth.sqlite"), "session", time.Hour, "", false, "integration-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	oauth, err := cxdbauth.NewOAuthServer(store, "https://cxdb.example", "/auth/login")
	if err != nil {
		t.Fatal(err)
	}

	redirectURI := "http://127.0.0.1:49152/callback"
	registrationBody := `{"client_name":"official MCP client","redirect_uris":["` + redirectURI + `"],"token_endpoint_auth_method":"none"}`
	registrationRequest := httptest.NewRequest(http.MethodPost, "/oauth/register", strings.NewReader(registrationBody))
	registrationResponse := httptest.NewRecorder()
	oauth.RegisterHandler(registrationResponse, registrationRequest)
	if registrationResponse.Code != http.StatusCreated {
		t.Fatalf("register status = %d, body=%s", registrationResponse.Code, registrationResponse.Body.String())
	}
	var registration struct {
		ClientID string `json:"client_id"`
	}
	if err := json.Unmarshal(registrationResponse.Body.Bytes(), &registration); err != nil {
		t.Fatal(err)
	}

	sessionID, err := store.CreateForIdentity(t.Context(), "https://id.example", "alice", "alice@example.com", "Alice", "", "oidc", []string{"cxdb:read", "cxdb:write"})
	if err != nil {
		t.Fatal(err)
	}
	verifier := strings.Repeat("v", 64)
	digest := sha256.Sum256([]byte(verifier))
	query := url.Values{
		"response_type":         {"code"},
		"client_id":             {registration.ClientID},
		"redirect_uri":          {redirectURI},
		"state":                 {"client-state"},
		"scope":                 {"cxdb:read cxdb:write"},
		"resource":              {"https://cxdb.example/mcp"},
		"code_challenge":        {base64.RawURLEncoding.EncodeToString(digest[:])},
		"code_challenge_method": {"S256"},
	}
	authorizeRequest := httptest.NewRequest(http.MethodGet, "/oauth/authorize?"+query.Encode(), nil)
	addSessionCookie(store, sessionID, authorizeRequest)
	authorizeResponse := httptest.NewRecorder()
	oauth.AuthorizeHandler(authorizeResponse, authorizeRequest)
	match := regexp.MustCompile(`name="request" value="([^"]+)"`).FindStringSubmatch(authorizeResponse.Body.String())
	if authorizeResponse.Code != http.StatusOK || len(match) != 2 {
		t.Fatalf("authorization consent status = %d, body=%s", authorizeResponse.Code, authorizeResponse.Body.String())
	}
	consentForm := url.Values{"request": {html.UnescapeString(match[1])}, "decision": {"allow"}}
	consentRequest := httptest.NewRequest(http.MethodPost, "/oauth/authorize", strings.NewReader(consentForm.Encode()))
	consentRequest.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	addSessionCookie(store, sessionID, consentRequest)
	consentResponse := httptest.NewRecorder()
	oauth.AuthorizeHandler(consentResponse, consentRequest)
	location, err := url.Parse(consentResponse.Header().Get("Location"))
	if err != nil || location.Query().Get("code") == "" {
		t.Fatalf("consent redirect = %q, err=%v", consentResponse.Header().Get("Location"), err)
	}
	tokenForm := url.Values{
		"grant_type":    {"authorization_code"},
		"code":          {location.Query().Get("code")},
		"client_id":     {registration.ClientID},
		"redirect_uri":  {redirectURI},
		"code_verifier": {verifier},
	}
	tokenRequest := httptest.NewRequest(http.MethodPost, "/oauth/token", strings.NewReader(tokenForm.Encode()))
	tokenRequest.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	tokenResponse := httptest.NewRecorder()
	oauth.TokenHandler(tokenResponse, tokenRequest)
	var tokenPayload struct {
		AccessToken string `json:"access_token"`
	}
	if tokenResponse.Code != http.StatusOK || json.Unmarshal(tokenResponse.Body.Bytes(), &tokenPayload) != nil || tokenPayload.AccessToken == "" {
		t.Fatalf("token response status = %d, body=%s", tokenResponse.Code, tokenResponse.Body.String())
	}

	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"contexts":[]}`)
	}))
	t.Cleanup(backend.Close)
	handler, err := New(backend.URL, "https://cxdb.example/.well-known/oauth-protected-resource/mcp", []cxdbauth.BearerTokenVerifier{oauth}, slog.Default())
	if err != nil {
		t.Fatal(err)
	}
	remote := httptest.NewServer(handler)
	t.Cleanup(remote.Close)
	client := mcp.NewClient(&mcp.Implementation{Name: "cxdb-oauth-test", Version: "1"}, nil)
	httpClient := &http.Client{Transport: bearerTransport{base: http.DefaultTransport, token: tokenPayload.AccessToken}}
	mcpSession, err := client.Connect(t.Context(), &mcp.StreamableClientTransport{Endpoint: remote.URL, HTTPClient: httpClient, DisableStandaloneSSE: true}, nil)
	if err != nil {
		t.Fatalf("OAuth-backed official MCP client handshake: %v", err)
	}
	defer func() { _ = mcpSession.Close() }()
	if _, err := mcpSession.CallTool(t.Context(), &mcp.CallToolParams{Name: "cxdb_list_contexts", Arguments: map[string]any{"limit": 1}}); err != nil {
		t.Fatalf("OAuth-backed read tool: %v", err)
	}
}

func addSessionCookie(store *cxdbauth.SessionStore, sessionID string, request *http.Request) {
	recorder := httptest.NewRecorder()
	store.SetCookie(recorder, sessionID)
	request.AddCookie(recorder.Result().Cookies()[0])
}
