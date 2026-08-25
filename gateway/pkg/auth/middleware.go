// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"log"
	"net"
	"net/http"
	"os"
	"strings"
	"time"
)

// BearerTokenVerifier validates bearer tokens and returns a session.
// Implemented by K8sOIDCVerifier and AWSTokenExchanger.
type BearerTokenVerifier interface {
	Verify(token string) (*Session, error)
}

// RequestTokenVerifier validates request-bound bearer token schemes like request-bound.
type RequestTokenVerifier interface {
	VerifyWithRequest(r *http.Request, token string) (*Session, error)
}

// Debug auth bypass configuration (set via environment variables)
// DEBUG_AUTH_TOKEN: Static token for Authorization header (e.g., "Bearer debug-token-123")
// DEBUG_AUTH_ALLOWED_IPS: Comma-separated list of allowed IPs (e.g., "107.131.127.143,10.0.0.1")
var (
	debugAuthToken      = os.Getenv("DEBUG_AUTH_TOKEN")
	debugAuthAllowedIPs = parseAllowedIPs(os.Getenv("DEBUG_AUTH_ALLOWED_IPS"))
)

func parseAllowedIPs(s string) map[string]bool {
	ips := make(map[string]bool)
	for _, ip := range strings.Split(s, ",") {
		ip = strings.TrimSpace(ip)
		if ip != "" {
			ips[ip] = true
		}
	}
	return ips
}

// getClientIP returns the TCP peer's address parsed from req.RemoteAddr.
//
// Sprint 019 / ADR-006: this function mirrors `proxy.observedPeerIP` and is
// the sole source of real-client-IP truth in the auth package. It NEVER reads
// `X-Forwarded-For` or `Forwarded` — those headers are attacker-controllable
// and would let a client spoof the debug-auth IP allowlist check.
func getClientIP(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

// checkDebugAuth checks if the request has a valid debug Authorization header
// from an allowed IP address. Returns a debug session if valid, nil otherwise.
func checkDebugAuth(r *http.Request) *Session {
	if debugAuthToken == "" {
		return nil // Debug auth not configured
	}

	// Check Authorization header
	auth := r.Header.Get("Authorization")
	if auth != debugAuthToken {
		return nil
	}

	// Check IP allowlist
	clientIP := getClientIP(r)
	if !debugAuthAllowedIPs[clientIP] {
		log.Printf("[auth] DEBUG_AUTH_TOKEN matched but IP %s not in allowlist", clientIP)
		return nil
	}

	log.Printf("[auth] debug auth bypass granted for IP %s", clientIP)
	return &Session{
		ID: "debug-auth-session", Email: "debug@localhost", Name: "Debug Auth User",
		Scopes: []string{"cxdb:read", "cxdb:write"}, Issuer: "cxdb:debug", Subject: "debug",
		CreatedAt: time.Now().UTC(), ExpiresAt: time.Now().Add(24 * time.Hour).UTC(), AuthMethod: "debug",
	}
}

// AuthMiddlewareOptions configures the auth middleware.
type AuthMiddlewareOptions struct {
	Store          *SessionStore
	DevBypass      bool
	TokenVerifiers []BearerTokenVerifier // Optional: K8s OIDC, AWS IAM, etc.
}

// RequireAuthForReads is an HTTP middleware that enforces a valid session for
// all GET requests except explicitly whitelisted paths. A separate middleware
// enforces authentication and scopes for non-GET methods.
func RequireAuthForReads(store *SessionStore, next http.Handler, devBypass bool) http.Handler {
	return RequireAuthForReadsWithOptions(AuthMiddlewareOptions{
		Store:     store,
		DevBypass: devBypass,
	}, next)
}

// RequireAuthForReadsWithOptions is like RequireAuthForReads but with additional options.
func RequireAuthForReadsWithOptions(opts AuthMiddlewareOptions, next http.Handler) http.Handler {
	store := opts.Store

	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path := r.URL.Path

		// Non-GET methods are checked by RequireAuthForWrites.
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			if store.Debug() {
				log.Printf("[auth] allowing write method %s %s", r.Method, path)
			}
			next.ServeHTTP(w, r)
			return
		}

		// Always allow public paths
		if isPublicPath(path) {
			if store.Debug() {
				log.Printf("[auth] public path %s", path)
			}
			next.ServeHTTP(w, r)
			return
		}

		var sess *Session
		if strings.TrimSpace(r.Header.Get("Authorization")) != "" {
			sess = checkDebugAuth(r)
			if sess == nil {
				token := extractBearerToken(r)
				if token == "" {
					http.Error(w, "unsupported authorization scheme", http.StatusUnauthorized)
					return
				}
				sess = verifyBearer(r, token, opts.TokenVerifiers)
				if sess == nil {
					http.Error(w, "invalid bearer token", http.StatusUnauthorized)
					return
				}
			}
		} else {
			sess, _ = store.SessionFromRequest(r.Context(), r)
		}

		// In DEV_MODE, allow requests without a browser session by
		// injecting a synthetic user. This is only enabled when the
		// server is started with DEV_MODE=true and PublicBaseURL is
		// pointing at localhost.
		if sess == nil && opts.DevBypass {
			if store.Debug() {
				log.Printf("[auth] DEV_MODE enabled, injecting dev session for %s", path)
			}
			email := strings.TrimSpace(os.Getenv("DEV_EMAIL"))
			if email == "" {
				email = "dev@localhost"
			}
			name := strings.TrimSpace(os.Getenv("DEV_NAME"))
			if name == "" {
				name = "Dev Mode User"
			}
			sess = &Session{
				ID: "dev-mode-session", Email: email, Name: name,
				Scopes: []string{"cxdb:read", "cxdb:write"}, Issuer: "cxdb:dev", Subject: email, AuthMethod: "dev",
				CreatedAt: time.Now().UTC(), ExpiresAt: time.Now().Add(store.TTL()).UTC(),
			}
		}

		if sess == nil {
			// For API requests, return 401 instead of redirect
			if isAPIRequest(r) {
				if store.Debug() {
					log.Printf("[auth] returning 401 for API request %s", path)
				}
				http.Error(w, "unauthorized", http.StatusUnauthorized)
				return
			}
			if store.Debug() {
				log.Printf("[auth] redirecting to /login from %s", path)
			}
			http.Redirect(w, r, "/login", http.StatusFound)
			return
		}
		if !sess.HasScope("cxdb:read") {
			http.Error(w, "insufficient scope: cxdb:read required", http.StatusForbidden)
			return
		}

		if store.Debug() {
			log.Printf("[auth] authorized %s as %s", path, sess.Email)
		}
		ctx := WithUser(r.Context(), sess)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// RequireAuthForWrites enforces authentication on mutating HTTP methods.
// POST/PUT/PATCH/DELETE require a valid principal with the "cxdb:write"
// scope. GET/HEAD/OPTIONS pass through to the read middleware.
func RequireAuthForWrites(opts AuthMiddlewareOptions, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Read-only and preflight methods always pass through.
		if r.Method == http.MethodGet || r.Method == http.MethodHead || r.Method == http.MethodOptions {
			next.ServeHTTP(w, r)
			return
		}

		// Allow only explicitly public write endpoints.
		if isPublicWritePath(r.URL.Path) {
			next.ServeHTTP(w, r)
			return
		}

		var sess *Session
		if strings.TrimSpace(r.Header.Get("Authorization")) != "" {
			sess = checkDebugAuth(r)
			if sess == nil {
				token := extractBearerToken(r)
				if token == "" {
					http.Error(w, "unsupported authorization scheme", http.StatusUnauthorized)
					return
				}
				sess = verifyBearer(r, token, opts.TokenVerifiers)
				if sess == nil {
					http.Error(w, "invalid bearer token", http.StatusUnauthorized)
					return
				}
			}
		} else {
			sess, _ = opts.Store.SessionFromRequest(r.Context(), r)
		}

		// Dev mode bypass.
		if sess == nil && opts.DevBypass {
			email := strings.TrimSpace(os.Getenv("DEV_EMAIL"))
			if email == "" {
				email = "dev@localhost"
			}
			name := strings.TrimSpace(os.Getenv("DEV_NAME"))
			if name == "" {
				name = "Dev Mode User"
			}
			sess = &Session{
				ID: "dev-mode-session", Email: email, Name: name,
				Scopes: []string{"cxdb:read", "cxdb:write"}, Issuer: "cxdb:dev", Subject: email, AuthMethod: "dev",
				CreatedAt: time.Now().UTC(), ExpiresAt: time.Now().Add(opts.Store.TTL()).UTC(),
			}
		}

		if sess == nil {
			http.Error(w, "authentication required for write operations", http.StatusUnauthorized)
			return
		}

		if !sess.HasScope("cxdb:write") {
			http.Error(w, "insufficient scope: cxdb:write required", http.StatusForbidden)
			return
		}

		ctx := WithUser(r.Context(), sess)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// extractBearerToken extracts a bearer token from the Authorization header.
func extractBearerToken(r *http.Request) string {
	auth := r.Header.Get("Authorization")
	if strings.HasPrefix(auth, "Bearer ") {
		return strings.TrimSpace(strings.TrimPrefix(auth, "Bearer "))
	}
	return ""
}

func verifyBearer(r *http.Request, token string, verifiers []BearerTokenVerifier) *Session {
	for _, verifier := range verifiers {
		var session *Session
		var err error
		if requestVerifier, ok := verifier.(RequestTokenVerifier); ok {
			session, err = requestVerifier.VerifyWithRequest(r, token)
		} else {
			session, err = verifier.Verify(token)
		}
		if err == nil && session != nil {
			return session
		}
	}
	return nil
}

// isAPIRequest returns true if the request appears to be an API request
// (should get 401 instead of redirect on auth failure).
func isAPIRequest(r *http.Request) bool {
	// Check for explicit API path
	if strings.HasPrefix(r.URL.Path, "/v1/") || strings.HasPrefix(r.URL.Path, "/api/") {
		return true
	}
	// Check for Authorization header (service-to-service)
	if r.Header.Get("Authorization") != "" {
		return true
	}
	// Check Accept header for JSON
	accept := r.Header.Get("Accept")
	if strings.Contains(accept, "application/json") && !strings.Contains(accept, "text/html") {
		return true
	}
	return false
}

// isPublicWritePath returns true for endpoints that intentionally allow
// unauthenticated write methods (POST/PUT/PATCH/DELETE).
func isPublicWritePath(path string) bool {
	path = strings.ToLower(path)

	// Health checks may use POST from some probes.
	if path == "/healthz" || path == "/readyz" {
		return true
	}

	if path == "/auth/aws/token" || path == "/oauth/register" || path == "/oauth/token" || path == "/mcp" {
		return true
	}

	return false
}

func isPublicPath(path string) bool {
	path = strings.ToLower(path)

	// Health checks and login page
	if path == "/healthz" || path == "/readyz" || path == "/favicon.ico" || path == "/login" {
		return true
	}
	if isExactPublicAuthPath(path) || path == "/.well-known/oauth-authorization-server" || path == "/.well-known/oauth-protected-resource/mcp" || path == "/mcp" || path == "/openapi.json" || path == "/openapi.yaml" || path == "/llms.txt" {
		return true
	}
	// Static assets required to render the login page (Next.js static export)
	if strings.HasPrefix(path, "/_next/") || strings.HasPrefix(path, "/static/") {
		return true
	}
	return false
}

func isExactPublicAuthPath(path string) bool {
	switch path {
	case "/auth/login", "/auth/google/login", "/auth/google/callback", "/auth/google/logout",
		"/auth/oidc/login", "/auth/oidc/callback", "/auth/aws/token",
		"/oauth/authorize", "/oauth/register", "/oauth/token":
		return true
	default:
		return false
	}
}
