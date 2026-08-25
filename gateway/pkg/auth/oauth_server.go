// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"html/template"
	"net"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"time"
)

const (
	oauthCodeTTL    = 5 * time.Minute
	oauthTokenTTL   = time.Hour
	maxOAuthClients = 1000
)

// OAuthServer is the CXDB OAuth 2.1 authorization server used by remote MCP clients.
type OAuthServer struct {
	store     *SessionStore
	issuer    string
	resource  string
	loginPath string
}

type oauthAuthorizationRequest struct {
	ClientID    string   `json:"client_id"`
	RedirectURI string   `json:"redirect_uri"`
	State       string   `json:"state"`
	Challenge   string   `json:"challenge"`
	Scopes      []string `json:"scopes"`
	Resource    string   `json:"resource"`
	ExpiresAt   int64    `json:"expires_at"`
}

// NewOAuthServer initializes additive OAuth tables.
func NewOAuthServer(store *SessionStore, publicBaseURL, loginPath string) (*OAuthServer, error) {
	issuer := strings.TrimSuffix(publicBaseURL, "/")
	s := &OAuthServer{store: store, issuer: issuer, resource: issuer + "/mcp", loginPath: loginPath}
	const schema = `
	CREATE TABLE IF NOT EXISTS oauth_clients (
		client_id TEXT PRIMARY KEY,
		client_name TEXT NOT NULL,
		redirect_uris_json TEXT NOT NULL,
		created_at TIMESTAMP NOT NULL
	);
	CREATE TABLE IF NOT EXISTS oauth_authorization_codes (
		code_hash TEXT PRIMARY KEY,
		client_id TEXT NOT NULL,
		redirect_uri TEXT NOT NULL,
		code_challenge TEXT NOT NULL,
		owner_issuer TEXT NOT NULL,
		owner_subject TEXT NOT NULL,
		email TEXT NOT NULL,
		scopes_json TEXT NOT NULL,
		expires_at TIMESTAMP NOT NULL,
		used_at TIMESTAMP
	);
	CREATE TABLE IF NOT EXISTS oauth_access_tokens (
		token_hash TEXT PRIMARY KEY,
		owner_issuer TEXT NOT NULL,
		owner_subject TEXT NOT NULL,
		email TEXT NOT NULL,
		scopes_json TEXT NOT NULL,
		created_at TIMESTAMP NOT NULL,
		expires_at TIMESTAMP NOT NULL,
		last_used_at TIMESTAMP,
		revoked_at TIMESTAMP
	);
	CREATE INDEX IF NOT EXISTS idx_oauth_access_tokens_owner ON oauth_access_tokens(owner_issuer, owner_subject);
	`
	if _, err := store.db.Exec(schema); err != nil {
		return nil, fmt.Errorf("initialize OAuth schema: %w", err)
	}
	return s, nil
}

// MetadataHandler serves RFC 8414 authorization-server metadata.
func (s *OAuthServer) MetadataHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"issuer":                                         s.issuer,
		"authorization_endpoint":                         s.issuer + "/oauth/authorize",
		"token_endpoint":                                 s.issuer + "/oauth/token",
		"registration_endpoint":                          s.issuer + "/oauth/register",
		"response_types_supported":                       []string{"code"},
		"grant_types_supported":                          []string{"authorization_code"},
		"code_challenge_methods_supported":               []string{"S256"},
		"token_endpoint_auth_methods_supported":          []string{"none"},
		"scopes_supported":                               []string{"cxdb:read", "cxdb:write"},
		"authorization_response_iss_parameter_supported": true,
	})
}

// RegisterHandler implements RFC 7591 dynamic client registration for public MCP clients.
func (s *OAuthServer) RegisterHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 64<<10)
	var request struct {
		ClientName              string   `json:"client_name"`
		RedirectURIs            []string `json:"redirect_uris"`
		TokenEndpointAuthMethod string   `json:"token_endpoint_auth_method"`
	}
	if err := json.NewDecoder(r.Body).Decode(&request); err != nil || len(request.RedirectURIs) == 0 || len(request.RedirectURIs) > 10 {
		oauthError(w, http.StatusBadRequest, "invalid_client_metadata", "redirect_uris is required")
		return
	}
	if request.TokenEndpointAuthMethod != "" && request.TokenEndpointAuthMethod != "none" {
		oauthError(w, http.StatusBadRequest, "invalid_client_metadata", "only public clients are supported")
		return
	}
	for _, redirectURI := range request.RedirectURIs {
		if err := validateOAuthRedirectURI(redirectURI); err != nil {
			oauthError(w, http.StatusBadRequest, "invalid_redirect_uri", err.Error())
			return
		}
	}
	if len(request.ClientName) > 120 {
		oauthError(w, http.StatusBadRequest, "invalid_client_metadata", "client_name is too long")
		return
	}
	if request.ClientName == "" {
		request.ClientName = "MCP client"
	}
	clientID, err := randomOpaque("mcpclient_", 24)
	if err != nil {
		http.Error(w, "registration failed", http.StatusInternalServerError)
		return
	}
	redirectJSON, _ := json.Marshal(request.RedirectURIs)
	result, err := s.store.db.ExecContext(r.Context(), `
		INSERT INTO oauth_clients (client_id, client_name, redirect_uris_json, created_at)
		SELECT ?, ?, ?, ?
		WHERE (SELECT COUNT(*) FROM oauth_clients) < ?
	`, clientID, request.ClientName, string(redirectJSON), time.Now().UTC(), maxOAuthClients)
	if err != nil {
		http.Error(w, "registration failed", http.StatusInternalServerError)
		return
	}
	rows, err := result.RowsAffected()
	if err != nil {
		http.Error(w, "registration failed", http.StatusInternalServerError)
		return
	}
	if rows != 1 {
		oauthError(w, http.StatusTooManyRequests, "registration_limit_reached", "the maximum number of registered clients has been reached")
		return
	}
	writeJSON(w, http.StatusCreated, map[string]any{
		"client_id":                  clientID,
		"client_name":                request.ClientName,
		"redirect_uris":              request.RedirectURIs,
		"token_endpoint_auth_method": "none",
		"grant_types":                []string{"authorization_code"},
		"response_types":             []string{"code"},
	})
}

// AuthorizeHandler validates an OAuth request, authenticates through the browser OIDC session, and asks for consent.
func (s *OAuthServer) AuthorizeHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method == http.MethodPost {
		s.completeAuthorization(w, r)
		return
	}
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	request, err := s.parseAuthorizationRequest(r.URL.Query())
	if err != nil {
		oauthError(w, http.StatusBadRequest, "invalid_request", err.Error())
		return
	}
	session, _ := s.store.SessionFromRequest(r.Context(), r)
	if session == nil {
		returnTo := r.URL.RequestURI()
		http.Redirect(w, r, s.loginPath+"?return_to="+url.QueryEscape(returnTo), http.StatusFound)
		return
	}
	signed, err := s.signAuthorizationRequest(request)
	if err != nil {
		http.Error(w, "unable to authorize", http.StatusInternalServerError)
		return
	}
	consentCSP, err := oauthConsentCSP(request.RedirectURI)
	if err != nil {
		http.Error(w, "unable to authorize", http.StatusInternalServerError)
		return
	}
	// Browsers apply form-action to redirects after form submission. Permit only
	// this registered OAuth callback so loopback MCP clients can receive the code.
	w.Header().Set("Content-Security-Policy", consentCSP)
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	_ = consentTemplate.Execute(w, struct {
		Client string
		Email  string
		Scopes string
		Token  string
	}{request.ClientID, session.Email, strings.Join(request.Scopes, ", "), signed})
}

func (s *OAuthServer) completeAuthorization(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		oauthError(w, http.StatusBadRequest, "invalid_request", "invalid form")
		return
	}
	request, err := s.verifyAuthorizationRequest(r.Form.Get("request"))
	if err != nil {
		oauthError(w, http.StatusBadRequest, "invalid_request", "invalid or expired authorization request")
		return
	}
	session, _ := s.store.SessionFromRequest(r.Context(), r)
	if session == nil {
		http.Error(w, "login required", http.StatusUnauthorized)
		return
	}
	if r.Form.Get("decision") != "allow" {
		s.redirectOAuth(w, r, request.RedirectURI, map[string]string{"error": "access_denied", "state": request.State})
		return
	}
	for _, scope := range request.Scopes {
		if !session.HasScope(scope) {
			s.redirectOAuth(w, r, request.RedirectURI, map[string]string{"error": "invalid_scope", "state": request.State})
			return
		}
	}
	code, err := randomOpaque("cxoc_", 32)
	if err != nil {
		http.Error(w, "unable to authorize", http.StatusInternalServerError)
		return
	}
	scopesJSON, _ := json.Marshal(request.Scopes)
	_, err = s.store.db.ExecContext(r.Context(), `
		INSERT INTO oauth_authorization_codes
		(code_hash, client_id, redirect_uri, code_challenge, owner_issuer, owner_subject, email, scopes_json, expires_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
	`, s.hash("code", code), request.ClientID, request.RedirectURI, request.Challenge, session.Issuer, session.Subject, session.Email, string(scopesJSON), time.Now().UTC().Add(oauthCodeTTL))
	if err != nil {
		http.Error(w, "unable to authorize", http.StatusInternalServerError)
		return
	}
	s.redirectOAuth(w, r, request.RedirectURI, map[string]string{"code": code, "state": request.State, "iss": s.issuer})
}

// TokenHandler exchanges a single-use authorization code using PKCE S256.
func (s *OAuthServer) TokenHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	if err := r.ParseForm(); err != nil || r.Form.Get("grant_type") != "authorization_code" {
		oauthError(w, http.StatusBadRequest, "unsupported_grant_type", "authorization_code is required")
		return
	}
	code := r.Form.Get("code")
	clientID := r.Form.Get("client_id")
	redirectURI := r.Form.Get("redirect_uri")
	verifier := r.Form.Get("code_verifier")
	if code == "" || clientID == "" || redirectURI == "" || verifier == "" {
		oauthError(w, http.StatusBadRequest, "invalid_request", "code, client_id, redirect_uri, and code_verifier are required")
		return
	}
	tx, err := s.store.db.BeginTx(r.Context(), nil)
	if err != nil {
		http.Error(w, "token exchange failed", http.StatusInternalServerError)
		return
	}
	defer tx.Rollback()
	var record struct {
		ClientID, RedirectURI, Challenge, Issuer, Subject, Email, ScopesJSON string
		ExpiresAt                                                            time.Time
		UsedAt                                                               sql.NullTime
	}
	err = tx.QueryRowContext(r.Context(), `SELECT client_id, redirect_uri, code_challenge, owner_issuer, owner_subject, email, scopes_json, expires_at, used_at FROM oauth_authorization_codes WHERE code_hash = ?`, s.hash("code", code)).Scan(
		&record.ClientID, &record.RedirectURI, &record.Challenge, &record.Issuer, &record.Subject, &record.Email, &record.ScopesJSON, &record.ExpiresAt, &record.UsedAt,
	)
	if err != nil || record.UsedAt.Valid || time.Now().After(record.ExpiresAt) || record.ClientID != clientID || record.RedirectURI != redirectURI || !verifyPKCES256(verifier, record.Challenge) {
		oauthError(w, http.StatusBadRequest, "invalid_grant", "authorization code is invalid")
		return
	}
	result, err := tx.ExecContext(r.Context(), `UPDATE oauth_authorization_codes SET used_at = ? WHERE code_hash = ? AND used_at IS NULL`, time.Now().UTC(), s.hash("code", code))
	if err != nil {
		http.Error(w, "token exchange failed", http.StatusInternalServerError)
		return
	}
	rows, _ := result.RowsAffected()
	if rows != 1 {
		oauthError(w, http.StatusBadRequest, "invalid_grant", "authorization code is invalid")
		return
	}
	accessToken, err := randomOpaque("cxoa_", 32)
	if err != nil {
		http.Error(w, "token exchange failed", http.StatusInternalServerError)
		return
	}
	now := time.Now().UTC()
	expires := now.Add(oauthTokenTTL)
	if _, err := tx.ExecContext(r.Context(), `INSERT INTO oauth_access_tokens (token_hash, owner_issuer, owner_subject, email, scopes_json, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)`, s.hash("access", accessToken), record.Issuer, record.Subject, record.Email, record.ScopesJSON, now, expires); err != nil {
		http.Error(w, "token exchange failed", http.StatusInternalServerError)
		return
	}
	if err := tx.Commit(); err != nil {
		http.Error(w, "token exchange failed", http.StatusInternalServerError)
		return
	}
	var scopes []string
	_ = json.Unmarshal([]byte(record.ScopesJSON), &scopes)
	writeJSON(w, http.StatusOK, map[string]any{"access_token": accessToken, "token_type": "Bearer", "expires_in": int(oauthTokenTTL.Seconds()), "scope": strings.Join(scopes, " ")})
}

// Verify implements BearerTokenVerifier for OAuth access tokens.
func (s *OAuthServer) Verify(token string) (*Session, error) {
	var session Session
	var scopesJSON string
	var revoked sql.NullTime
	err := s.store.db.QueryRow(`SELECT owner_issuer, owner_subject, email, scopes_json, expires_at, revoked_at FROM oauth_access_tokens WHERE token_hash = ?`, s.hash("access", token)).Scan(&session.Issuer, &session.Subject, &session.Email, &scopesJSON, &session.ExpiresAt, &revoked)
	if err != nil || revoked.Valid || time.Now().After(session.ExpiresAt) {
		return nil, errors.New("invalid OAuth access token")
	}
	if err := json.Unmarshal([]byte(scopesJSON), &session.Scopes); err != nil {
		return nil, errors.New("invalid OAuth access token scopes")
	}
	session.ID = "oauth:" + session.Issuer + ":" + session.Subject
	session.Name = session.Email
	session.AuthMethod = "oauth_access_token"
	_, _ = s.store.db.Exec(`UPDATE oauth_access_tokens SET last_used_at = ? WHERE token_hash = ?`, time.Now().UTC(), s.hash("access", token))
	return &session, nil
}

func (s *OAuthServer) parseAuthorizationRequest(values url.Values) (oauthAuthorizationRequest, error) {
	request := oauthAuthorizationRequest{
		ClientID: values.Get("client_id"), RedirectURI: values.Get("redirect_uri"), State: values.Get("state"),
		Challenge: values.Get("code_challenge"), Resource: values.Get("resource"), ExpiresAt: time.Now().Add(10 * time.Minute).Unix(),
	}
	if values.Get("response_type") != "code" || request.ClientID == "" || request.RedirectURI == "" || request.State == "" || request.Challenge == "" || values.Get("code_challenge_method") != "S256" {
		return request, errors.New("response_type=code, state, and PKCE S256 are required")
	}
	if request.Resource != "" && request.Resource != s.resource {
		return request, errors.New("resource must identify the CXDB MCP endpoint")
	}
	request.Resource = s.resource
	request.Scopes = strings.Fields(values.Get("scope"))
	if len(request.Scopes) == 0 {
		request.Scopes = []string{"cxdb:read"}
	}
	for _, scope := range request.Scopes {
		if scope != "cxdb:read" && scope != "cxdb:write" {
			return request, fmt.Errorf("unsupported scope %q", scope)
		}
	}
	if !slices.Contains(request.Scopes, "cxdb:read") {
		return request, errors.New("cxdb:read scope is required")
	}
	var redirectsJSON string
	if err := s.store.db.QueryRow(`SELECT redirect_uris_json FROM oauth_clients WHERE client_id = ?`, request.ClientID).Scan(&redirectsJSON); err != nil {
		return request, errors.New("unknown client_id")
	}
	var redirects []string
	if json.Unmarshal([]byte(redirectsJSON), &redirects) != nil || !slices.Contains(redirects, request.RedirectURI) {
		return request, errors.New("redirect_uri is not registered")
	}
	return request, nil
}

func (s *OAuthServer) signAuthorizationRequest(request oauthAuthorizationRequest) (string, error) {
	encoded, err := json.Marshal(request)
	if err != nil {
		return "", err
	}
	return s.store.sign(base64.RawURLEncoding.EncodeToString(encoded)), nil
}

func (s *OAuthServer) verifyAuthorizationRequest(signed string) (oauthAuthorizationRequest, error) {
	var request oauthAuthorizationRequest
	encoded, ok := s.store.verify(signed)
	if !ok {
		return request, errors.New("bad signature")
	}
	raw, err := base64.RawURLEncoding.DecodeString(encoded)
	if err != nil {
		return request, err
	}
	if err := json.Unmarshal(raw, &request); err != nil {
		return request, err
	}
	if request.ExpiresAt < time.Now().Unix() {
		return request, errors.New("expired request")
	}
	return request, nil
}

func (s *OAuthServer) redirectOAuth(w http.ResponseWriter, r *http.Request, redirectURI string, params map[string]string) {
	u, err := url.Parse(redirectURI)
	if err != nil {
		http.Error(w, "invalid redirect URI", http.StatusBadRequest)
		return
	}
	query := u.Query()
	for key, value := range params {
		if value != "" {
			query.Set(key, value)
		}
	}
	u.RawQuery = query.Encode()
	http.Redirect(w, r, u.String(), http.StatusFound)
}

func (s *OAuthServer) hash(kind, value string) string {
	mac := hmac.New(sha256.New, s.store.secret)
	_, _ = mac.Write([]byte("cxdb-oauth-" + kind + "\x00" + value))
	return hex.EncodeToString(mac.Sum(nil))
}

func validateOAuthRedirectURI(raw string) error {
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" || u.Fragment != "" || u.User != nil {
		return errors.New("redirect URI must be absolute and have no fragment or userinfo")
	}
	if u.Scheme == "https" {
		return nil
	}
	if u.Scheme != "http" {
		return errors.New("redirect URI must use HTTPS or loopback HTTP")
	}
	host := u.Hostname()
	if host != "localhost" {
		ip := net.ParseIP(host)
		if ip == nil || !ip.IsLoopback() {
			return errors.New("HTTP redirect URI must use a loopback host")
		}
	}
	return nil
}

func oauthConsentCSP(redirectURI string) (string, error) {
	if err := validateOAuthRedirectURI(redirectURI); err != nil {
		return "", err
	}
	u, err := url.Parse(redirectURI)
	if err != nil {
		return "", err
	}
	origin := (&url.URL{Scheme: u.Scheme, Host: u.Host}).String()
	return "default-src 'none'; form-action 'self' " + origin + "; frame-ancestors 'none'; base-uri 'none'", nil
}

func verifyPKCES256(verifier, challenge string) bool {
	digest := sha256.Sum256([]byte(verifier))
	return hmac.Equal([]byte(base64.RawURLEncoding.EncodeToString(digest[:])), []byte(challenge))
}

func randomOpaque(prefix string, size int) (string, error) {
	b := make([]byte, size)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return prefix + base64.RawURLEncoding.EncodeToString(b), nil
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}

func oauthError(w http.ResponseWriter, status int, code, description string) {
	writeJSON(w, status, map[string]string{"error": code, "error_description": description})
}

var consentTemplate = template.Must(template.New("consent").Parse(`<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Authorize CXDB</title></head>
<body><main><h1>Authorize CXDB MCP access</h1><p><strong>{{.Client}}</strong> requests {{.Scopes}} as {{.Email}}.</p>
<form method="post"><input type="hidden" name="request" value="{{.Token}}"><button name="decision" value="allow">Allow</button><button name="decision" value="deny">Deny</button></form>
</main></body></html>`))

// VerifyOAuthTokenWithContext is useful to adapters that need request context.
func (s *OAuthServer) VerifyOAuthTokenWithContext(_ context.Context, token string) (*Session, error) {
	return s.Verify(token)
}
