// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	_ "github.com/mattn/go-sqlite3"
)

// Session captures the authenticated user for a browser.
type Session struct {
	ID         string
	Email      string
	Name       string
	Picture    string
	Scopes     []string
	CreatedAt  time.Time
	ExpiresAt  time.Time
	AuthMethod string // Authentication method, for example "oidc" or "k8s_oidc".
	Issuer     string // Token issuer URL
	Subject    string // Stable subject within Issuer
}

// HasScope returns true if the session includes the given scope.
func (s *Session) HasScope(scope string) bool {
	for _, sc := range s.Scopes {
		if sc == scope {
			return true
		}
	}
	return false
}

// IsAPIToken reports whether this session came from a personal API token.
// Handlers can use this to prevent a bearer token from creating or revoking
// other personal credentials.
func (s *Session) IsAPIToken() bool {
	return s != nil && s.AuthMethod == APITokenAuthMethod
}

// SessionStore handles persistence of sessions in SQLite and
// issuing/clearing the browser cookie.
type SessionStore struct {
	db         *sql.DB
	ttl        time.Duration
	cookieName string
	domain     string
	secure     bool
	secret     []byte
	debug      bool
}

func NewSessionStore(databasePath, cookieName string, ttl time.Duration, cookieDomain string, secure bool, secret string) (*SessionStore, error) {
	if err := os.MkdirAll(filepath.Dir(databasePath), 0o755); err != nil {
		return nil, fmt.Errorf("create data dir: %w", err)
	}
	db, err := sql.Open("sqlite3", databasePath)
	if err != nil {
		return nil, fmt.Errorf("open sqlite: %w", err)
	}

	// Enable WAL mode for better durability in single-writer scenarios
	if _, err := db.Exec("PRAGMA journal_mode=WAL"); err != nil {
		return nil, fmt.Errorf("enable WAL mode: %w", err)
	}

	store := &SessionStore{
		db:         db,
		ttl:        ttl,
		cookieName: cookieName,
		domain:     strings.TrimSpace(cookieDomain),
		secure:     secure,
		secret:     []byte(secret),
		debug:      strings.Contains(os.Getenv("DEBUG"), "auth") || strings.Contains(os.Getenv("DEBUG"), "all"),
	}
	if err := store.ensureSchema(); err != nil {
		return nil, err
	}
	return store, nil
}

func (s *SessionStore) ensureSchema() error {
	const schema = `
	CREATE TABLE IF NOT EXISTS sessions (
		id TEXT PRIMARY KEY,
		email TEXT NOT NULL,
		name TEXT,
		picture TEXT,
		created_at TIMESTAMP NOT NULL,
		expires_at TIMESTAMP NOT NULL
	);
	CREATE INDEX IF NOT EXISTS idx_sessions_email ON sessions(email);
	CREATE TABLE IF NOT EXISTS api_tokens (
		id TEXT PRIMARY KEY,
		name TEXT NOT NULL,
		issuer TEXT NOT NULL,
		subject TEXT NOT NULL,
		scopes TEXT NOT NULL,
		token_hash TEXT NOT NULL UNIQUE,
		created_at TIMESTAMP NOT NULL,
		expires_at TIMESTAMP NOT NULL,
		revoked_at TIMESTAMP,
		last_used_at TIMESTAMP
	);
	CREATE INDEX IF NOT EXISTS idx_api_tokens_owner ON api_tokens(issuer, subject);
	CREATE INDEX IF NOT EXISTS idx_api_tokens_hash ON api_tokens(token_hash);
	`
	if _, err := s.db.Exec(schema); err != nil {
		return fmt.Errorf("init schema: %w", err)
	}
	// Backfill for older schemas missing the picture column; ignore duplicate errors.
	_, _ = s.db.Exec(`ALTER TABLE sessions ADD COLUMN picture TEXT;`)
	// These columns are deliberately nullable so that existing installations can
	// be upgraded without rewriting or invalidating their browser sessions.
	for _, statement := range []string{
		`ALTER TABLE sessions ADD COLUMN issuer TEXT`,
		`ALTER TABLE sessions ADD COLUMN subject TEXT`,
		`ALTER TABLE sessions ADD COLUMN scopes TEXT`,
		`ALTER TABLE sessions ADD COLUMN auth_method TEXT`,
	} {
		_, _ = s.db.Exec(statement)
	}
	return nil
}

// Create inserts a new session and returns its ID.
func (s *SessionStore) Create(ctx context.Context, email, name, picture string) (string, error) {
	return s.CreateForIdentity(ctx, "https://accounts.google.com", email, email, name, picture, "google_oauth", []string{"cxdb:read", "cxdb:write"})
}

// CreateForIdentity inserts a browser session with a stable issuer/subject
// identity and authorization scopes. Create remains the compatibility API for
// callers that only have a Google profile.
func (s *SessionStore) CreateForIdentity(ctx context.Context, issuer, subject, email, name, picture, authMethod string, scopes []string) (string, error) {
	id, err := randomID()
	if err != nil {
		return "", err
	}
	now := time.Now().UTC()
	expires := now.Add(s.ttl)
	scopeJSON, err := json.Marshal(normalizeScopes(scopes))
	if err != nil {
		return "", fmt.Errorf("encode session scopes: %w", err)
	}
	_, err = s.db.ExecContext(ctx, `
		INSERT INTO sessions (id, email, name, picture, issuer, subject, scopes, auth_method, created_at, expires_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
	`, id, email, name, picture, strings.TrimSpace(issuer), strings.TrimSpace(subject), string(scopeJSON), strings.TrimSpace(authMethod), now, expires)
	if err != nil {
		return "", fmt.Errorf("insert session: %w", err)
	}
	return id, nil
}

// CreateWithIdentity is retained as a convenience for callers using the
// original field-oriented order introduced during the identity migration.
func (s *SessionStore) CreateWithIdentity(ctx context.Context, email, name, picture, issuer, subject string, scopes []string, authMethod string) (string, error) {
	return s.CreateForIdentity(ctx, issuer, subject, email, name, picture, authMethod, scopes)
}

// Get returns a valid, non-expired session by ID.
func (s *SessionStore) Get(ctx context.Context, id string) (*Session, error) {
	row := s.db.QueryRowContext(ctx, `
		SELECT id, email, name, picture, issuer, subject, scopes, auth_method, created_at, expires_at
		FROM sessions
		WHERE id = ?
	`, id)

	var sess Session
	var email, name, picture, issuer, subject, scopesJSON, authMethod sql.NullString
	if err := row.Scan(&sess.ID, &email, &name, &picture, &issuer, &subject, &scopesJSON, &authMethod, &sess.CreatedAt, &sess.ExpiresAt); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return nil, nil
		}
		return nil, fmt.Errorf("select session: %w", err)
	}
	sess.Email, sess.Name, sess.Picture = email.String, name.String, picture.String
	sess.Issuer, sess.Subject, sess.AuthMethod = issuer.String, subject.String, authMethod.String
	if scopesJSON.Valid && scopesJSON.String != "" {
		if err := json.Unmarshal([]byte(scopesJSON.String), &sess.Scopes); err != nil {
			return nil, fmt.Errorf("decode session scopes: %w", err)
		}
	}
	// Sessions that predate the identity migration were authenticated Google
	// browser sessions. Preserve them until their normal expiry.
	if !issuer.Valid && !subject.Valid && !scopesJSON.Valid && !authMethod.Valid {
		sess.Issuer = "https://accounts.google.com"
		sess.Subject = sess.Email
		sess.AuthMethod = "google_oauth"
		sess.Scopes = []string{"cxdb:read", "cxdb:write"}
	}
	if time.Now().After(sess.ExpiresAt) {
		_ = s.Delete(ctx, id)
		return nil, nil
	}
	return &sess, nil
}

// Delete removes a session by ID.
func (s *SessionStore) Delete(ctx context.Context, id string) error {
	if _, err := s.db.ExecContext(ctx, `DELETE FROM sessions WHERE id = ?`, id); err != nil {
		return fmt.Errorf("delete session: %w", err)
	}
	return nil
}

// Close closes the underlying database handle.
func (s *SessionStore) Close() error {
	return s.db.Close()
}

// Ping verifies the underlying SQLite database is reachable.
func (s *SessionStore) Ping(ctx context.Context) error {
	return s.db.PingContext(ctx)
}

// SessionFromRequest fetches the session for the incoming HTTP request.
func (s *SessionStore) SessionFromRequest(ctx context.Context, r *http.Request) (*Session, error) {
	cookie, err := r.Cookie(s.cookieName)
	if err != nil {
		if s.debug {
			log.Printf("[auth] no session cookie on %s", r.URL.Path)
		}
		return nil, nil
	}
	value := strings.TrimSpace(cookie.Value)
	value, ok := s.verify(value)
	if !ok {
		if s.debug {
			log.Printf("[auth] bad signature for cookie on %s", r.URL.Path)
		}
		return nil, nil
	}
	if value == "" {
		if s.debug {
			log.Printf("[auth] empty cookie on %s", r.URL.Path)
		}
		return nil, nil
	}
	if s.debug {
		log.Printf("[auth] checking session %s", value)
	}
	return s.Get(ctx, value)
}

// SetCookie writes the session cookie using security best practices.
func (s *SessionStore) SetCookie(w http.ResponseWriter, sessionID string) {
	signed := s.sign(sessionID)
	http.SetCookie(w, &http.Cookie{
		Name:     s.cookieName,
		Value:    signed,
		Domain:   s.domain,
		Path:     "/",
		HttpOnly: true,
		Secure:   s.secure,
		SameSite: http.SameSiteLaxMode,
	})
}

// CSRFToken returns a session-bound token for browser credential-management requests.
func (s *SessionStore) CSRFToken(session *Session) string {
	if session == nil || session.ID == "" {
		return ""
	}
	mac := hmac.New(sha256.New, s.secret)
	_, _ = mac.Write([]byte("cxdb-csrf\x00" + session.ID))
	return hex.EncodeToString(mac.Sum(nil))
}

// ValidCSRFToken checks a session-bound CSRF token in constant time.
func (s *SessionStore) ValidCSRFToken(session *Session, token string) bool {
	expected := s.CSRFToken(session)
	return expected != "" && hmac.Equal([]byte(expected), []byte(strings.TrimSpace(token)))
}

// ClearCookie removes the session cookie from the browser.
func (s *SessionStore) ClearCookie(w http.ResponseWriter) {
	http.SetCookie(w, &http.Cookie{
		Name:     s.cookieName,
		Value:    "",
		Domain:   s.domain,
		Path:     "/",
		HttpOnly: true,
		Secure:   s.secure,
		SameSite: http.SameSiteLaxMode,
		MaxAge:   -1,
	})
}

// Domain returns the cookie domain for this session store.
func (s *SessionStore) Domain() string {
	return s.domain
}

// Secure returns whether cookies are marked secure.
func (s *SessionStore) Secure() bool {
	return s.secure
}

// TTL returns the session time-to-live.
func (s *SessionStore) TTL() time.Duration {
	return s.ttl
}

// Debug returns whether debug logging is enabled.
func (s *SessionStore) Debug() bool {
	return s.debug
}

func randomID() (string, error) {
	var b [32]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", fmt.Errorf("rand: %w", err)
	}
	return hex.EncodeToString(b[:]), nil
}

func (s *SessionStore) sign(value string) string {
	h := hmac.New(sha256.New, s.secret)
	h.Write([]byte(value))
	return value + "." + hex.EncodeToString(h.Sum(nil))
}

func (s *SessionStore) verify(raw string) (string, bool) {
	parts := strings.Split(raw, ".")
	if len(parts) < 2 {
		return "", false
	}
	value := strings.Join(parts[:len(parts)-1], ".")
	sig := parts[len(parts)-1]

	expected := s.sign(value)
	return value, subtleEqual(expected, raw) && subtleEqual(sig, expected[strings.LastIndex(expected, ".")+1:])
}

func subtleEqual(a, b string) bool {
	if len(a) != len(b) {
		return false
	}
	var diff byte
	for i := 0; i < len(a); i++ {
		diff |= a[i] ^ b[i]
	}
	return diff == 0
}
