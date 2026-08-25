// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"crypto"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/coreos/go-oidc/v3/oidc"
	"github.com/lestrrat-go/jwx/v2/jwk"
)

func signRS256JWT(t *testing.T, privateKey *rsa.PrivateKey, kid string, claims map[string]any) string {
	t.Helper()
	headerJSON, err := json.Marshal(map[string]any{"alg": "RS256", "typ": "JWT", "kid": kid})
	if err != nil {
		t.Fatal(err)
	}
	claimsJSON, err := json.Marshal(claims)
	if err != nil {
		t.Fatal(err)
	}
	b64 := base64.RawURLEncoding
	input := b64.EncodeToString(headerJSON) + "." + b64.EncodeToString(claimsJSON)
	sum := sha256.Sum256([]byte(input))
	signature, err := rsa.SignPKCS1v15(rand.Reader, privateKey, crypto.SHA256, sum[:])
	if err != nil {
		t.Fatal(err)
	}
	return input + "." + b64.EncodeToString(signature)
}

func TestBrowserOIDCVerifiedCodeFlowAndNonce(t *testing.T) {
	privateKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	key, err := jwk.FromRaw(&privateKey.PublicKey)
	if err != nil {
		t.Fatal(err)
	}
	const kid = "browser-test-key"
	_ = key.Set(jwk.KeyIDKey, kid)
	_ = key.Set(jwk.KeyUsageKey, "sig")
	_ = key.Set(jwk.AlgorithmKey, "RS256")
	set := jwk.NewSet()
	set.AddKey(key)
	jwks, _ := json.Marshal(set)

	var issuer string
	var tokenNonce string
	mux := http.NewServeMux()
	mux.HandleFunc("/.well-known/openid-configuration", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{
			"issuer": issuer, "authorization_endpoint": issuer + "/authorize", "token_endpoint": issuer + "/token", "jwks_uri": issuer + "/jwks",
			"response_types_supported": []string{"code"}, "subject_types_supported": []string{"public"}, "id_token_signing_alg_values_supported": []string{"RS256"},
		})
	})
	mux.HandleFunc("/jwks", func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write(jwks) })
	mux.HandleFunc("/token", func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil || r.Form.Get("code_verifier") == "" {
			http.Error(w, "PKCE required", http.StatusBadRequest)
			return
		}
		now := time.Now().Unix()
		idToken := signRS256JWT(t, privateKey, kid, map[string]any{
			"iss": issuer, "sub": "user-123", "aud": "cxdb-client", "exp": now + 300, "iat": now,
			"nonce": tokenNonce, "email": "alice@example.com", "email_verified": true, "name": "Alice",
		})
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "upstream", "token_type": "Bearer", "expires_in": 300, "id_token": idToken})
	})
	provider := httptest.NewTLSServer(mux)
	defer provider.Close()
	issuer = provider.URL

	store, err := NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", true, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	discoveryContext := oidc.ClientContext(context.Background(), provider.Client())
	browser, err := NewBrowserOIDC(discoveryContext, issuer, "cxdb-client", "client-secret", "https://cxdb.example", []string{"example.com"}, store)
	if err != nil {
		t.Fatal(err)
	}

	loginRequest := httptest.NewRequest(http.MethodGet, "/auth/oidc/login?return_to=%2Foauth%2Fauthorize%3Fclient_id%3Dtest", nil)
	loginResponse := httptest.NewRecorder()
	browser.LoginHandler(loginResponse, loginRequest)
	if loginResponse.Code != http.StatusFound || !strings.Contains(loginResponse.Header().Get("Location"), "code_challenge_method=S256") {
		t.Fatalf("login response = %d %s", loginResponse.Code, loginResponse.Header().Get("Location"))
	}
	transactionCookie := loginResponse.Result().Cookies()[0]
	txRequest := httptest.NewRequest(http.MethodGet, "/", nil)
	txRequest.AddCookie(transactionCookie)
	transaction, err := browser.transaction(txRequest)
	if err != nil {
		t.Fatal(err)
	}
	tokenNonce = transaction.Nonce

	callbackRequest := httptest.NewRequest(http.MethodGet, "/auth/oidc/callback?state="+url.QueryEscape(transaction.State)+"&code=test-code", nil)
	callbackRequest = callbackRequest.WithContext(oidc.ClientContext(callbackRequest.Context(), provider.Client()))
	callbackRequest.AddCookie(transactionCookie)
	callbackResponse := httptest.NewRecorder()
	browser.CallbackHandler(callbackResponse, callbackRequest)
	if callbackResponse.Code != http.StatusFound || callbackResponse.Header().Get("Location") != "/oauth/authorize?client_id=test" {
		t.Fatalf("callback response = %d location=%q body=%s", callbackResponse.Code, callbackResponse.Header().Get("Location"), callbackResponse.Body.String())
	}
	var sessionCookie *http.Cookie
	for _, cookie := range callbackResponse.Result().Cookies() {
		if cookie.Name == "session" {
			sessionCookie = cookie
		}
	}
	if sessionCookie == nil {
		t.Fatal("verified OIDC flow did not create a session")
	}
	sessionRequest := httptest.NewRequest(http.MethodGet, "/", nil)
	sessionRequest.AddCookie(sessionCookie)
	session, err := store.SessionFromRequest(context.Background(), sessionRequest)
	if err != nil || session == nil || session.Issuer != issuer || session.Subject != "user-123" || !session.HasScope("cxdb:write") {
		t.Fatalf("session = %+v, err=%v", session, err)
	}

	badLoginResponse := httptest.NewRecorder()
	browser.LoginHandler(badLoginResponse, httptest.NewRequest(http.MethodGet, "/auth/oidc/login", nil))
	badCookie := badLoginResponse.Result().Cookies()[0]
	badTxRequest := httptest.NewRequest(http.MethodGet, "/", nil)
	badTxRequest.AddCookie(badCookie)
	badTransaction, err := browser.transaction(badTxRequest)
	if err != nil {
		t.Fatal(err)
	}
	tokenNonce = "wrong-nonce"
	badCallback := httptest.NewRequest(http.MethodGet, "/auth/oidc/callback?state="+url.QueryEscape(badTransaction.State)+"&code=test-code", nil)
	badCallback = badCallback.WithContext(oidc.ClientContext(badCallback.Context(), provider.Client()))
	badCallback.AddCookie(badCookie)
	badCallbackResponse := httptest.NewRecorder()
	browser.CallbackHandler(badCallbackResponse, badCallback)
	if badCallbackResponse.Header().Get("Location") != "/login?error=unauthorized" {
		t.Fatalf("nonce mismatch redirect = %q", badCallbackResponse.Header().Get("Location"))
	}
}

func TestSafeLocalReturnRejectsBrowserNormalizedExternalPaths(t *testing.T) {
	for _, raw := range []string{
		`/%5cevil.com`,
		`/%5Cevil.com`,
		`/\evil.com`,
		`/%2f%2fevil.com`,
		`//evil.com`,
	} {
		if got := safeLocalReturn(raw); got != "" {
			t.Errorf("safeLocalReturn(%q) = %q, want rejection", raw, got)
		}
	}
}
