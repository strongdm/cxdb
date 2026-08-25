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
	"net/http"
	"strings"
	"time"
)

const (
	APITokenAuthMethod = "api_token"
	apiTokenPrefix     = "cxpat_"
)

var (
	ErrAPITokenNotFound = errors.New("api token not found")
	ErrAPITokenExpired  = errors.New("api token expired")
	ErrAPITokenRevoked  = errors.New("api token revoked")
)

// APIToken is the non-secret representation of a personal API token. It is
// safe to return from list and revoke APIs. The token plaintext and its hash
// are intentionally not fields on this type.
type APIToken struct {
	ID         string     `json:"id"`
	Prefix     string     `json:"prefix"` // Public identifier, also equal to ID.
	Name       string     `json:"name"`
	Issuer     string     `json:"issuer"`
	Subject    string     `json:"subject"`
	Scopes     []string   `json:"scopes"`
	CreatedAt  time.Time  `json:"created_at"`
	ExpiresAt  time.Time  `json:"expires_at"`
	RevokedAt  *time.Time `json:"revoked_at,omitempty"`
	LastUsedAt *time.Time `json:"last_used_at,omitempty"`
}

// APITokenCreateRequest describes a token and its stable owner. Issuer and
// Subject together identify the owner; neither email nor display name does.
type APITokenCreateRequest struct {
	Name      string
	Issuer    string
	Subject   string
	Scopes    []string
	ExpiresAt time.Time
}

// PersonalAPIToken and CreateAPITokenRequest are descriptive aliases for
// callers that use the personal-token terminology.
type PersonalAPIToken = APIToken
type CreateAPITokenRequest = APITokenCreateRequest

// CreateAPIToken creates a token and returns its metadata and plaintext. The
// plaintext is returned only by this call and is never persisted.
func (s *SessionStore) CreateAPIToken(ctx context.Context, req APITokenCreateRequest) (*APIToken, string, error) {
	if len(s.secret) == 0 {
		return nil, "", errors.New("token hashing secret is required")
	}
	issuer := strings.TrimSpace(req.Issuer)
	subject := strings.TrimSpace(req.Subject)
	if issuer == "" || subject == "" {
		return nil, "", errors.New("token issuer and subject are required")
	}
	name := strings.TrimSpace(req.Name)
	if name == "" {
		return nil, "", errors.New("token name is required")
	}
	scopes, err := validateAPITokenScopes(req.Scopes)
	if err != nil {
		return nil, "", err
	}

	// A zero expiry uses the store's configured lifetime. An explicitly past
	// expiry is retained so that callers can consistently test and inspect the
	// expiry/revocation lifecycle; verification rejects it.
	expiresAt := req.ExpiresAt
	if expiresAt.IsZero() {
		expiresAt = time.Now().UTC().Add(s.ttl)
	}
	now := time.Now().UTC()
	publicID, err := randomTokenPublicID()
	if err != nil {
		return nil, "", err
	}
	secret := make([]byte, 32) // 256 bits of entropy, encoded below.
	if _, err := rand.Read(secret); err != nil {
		return nil, "", fmt.Errorf("generate token secret: %w", err)
	}
	plaintext := publicID + "." + base64.RawURLEncoding.EncodeToString(secret)
	hash := s.apiTokenHash(plaintext)
	scopesJSON, err := marshalScopes(scopes)
	if err != nil {
		return nil, "", err
	}
	_, err = s.db.ExecContext(ctx, `
		INSERT INTO api_tokens
			(id, name, issuer, subject, scopes, token_hash, created_at, expires_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?)
	`, publicID, name, issuer, subject, scopesJSON, hash, now, expiresAt.UTC())
	if err != nil {
		return nil, "", fmt.Errorf("insert api token: %w", err)
	}
	return &APIToken{
		ID: publicID, Prefix: publicID, Name: name, Issuer: issuer, Subject: subject,
		Scopes: scopes, CreatedAt: now, ExpiresAt: expiresAt.UTC(),
	}, plaintext, nil
}

// CreatePersonalAPIToken is a handler-friendly form of CreateAPIToken that
// takes ownership from the authenticated session.
func (s *SessionStore) CreatePersonalAPIToken(ctx context.Context, sess *Session, name string, scopes []string, expiresAt time.Time) (*APIToken, string, error) {
	if sess == nil {
		return nil, "", errors.New("authenticated session is required")
	}
	return s.CreateAPIToken(ctx, APITokenCreateRequest{
		Name: name, Issuer: sess.Issuer, Subject: sess.Subject,
		Scopes: scopes, ExpiresAt: expiresAt,
	})
}

// ListAPITokens lists all tokens owned by issuer+subject. It never returns
// token plaintext or hashes.
func (s *SessionStore) ListAPITokens(ctx context.Context, issuer, subject string) ([]APIToken, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT id, name, issuer, subject, scopes, created_at, expires_at, revoked_at, last_used_at
		FROM api_tokens
		WHERE issuer = ? AND subject = ?
		ORDER BY created_at DESC, id DESC
	`, strings.TrimSpace(issuer), strings.TrimSpace(subject))
	if err != nil {
		return nil, fmt.Errorf("list api tokens: %w", err)
	}
	defer rows.Close()
	result := make([]APIToken, 0)
	for rows.Next() {
		var token APIToken
		var scopesJSON string
		var revokedAt, lastUsedAt sql.NullTime
		if err := rows.Scan(&token.ID, &token.Name, &token.Issuer, &token.Subject, &scopesJSON,
			&token.CreatedAt, &token.ExpiresAt, &revokedAt, &lastUsedAt); err != nil {
			return nil, fmt.Errorf("scan api token: %w", err)
		}
		token.Prefix = token.ID
		var err error
		token.Scopes, err = unmarshalScopes(scopesJSON)
		if err != nil {
			return nil, fmt.Errorf("decode api token scopes: %w", err)
		}
		if revokedAt.Valid {
			v := revokedAt.Time
			token.RevokedAt = &v
		}
		if lastUsedAt.Valid {
			v := lastUsedAt.Time
			token.LastUsedAt = &v
		}
		result = append(result, token)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate api tokens: %w", err)
	}
	return result, nil
}

// ListPersonalAPITokens lists only tokens owned by the authenticated session.
func (s *SessionStore) ListPersonalAPITokens(ctx context.Context, sess *Session) ([]APIToken, error) {
	if sess == nil {
		return nil, errors.New("authenticated session is required")
	}
	return s.ListAPITokens(ctx, sess.Issuer, sess.Subject)
}

// RevokeAPIToken revokes a token owned by issuer+subject. The id may be the
// public token id or the complete token plaintext. It is idempotent for an
// already-revoked token owned by the caller.
func (s *SessionStore) RevokeAPIToken(ctx context.Context, issuer, subject, id string) error {
	id = apiTokenPublicID(id)
	if id == "" {
		return ErrAPITokenNotFound
	}
	result, err := s.db.ExecContext(ctx, `
		UPDATE api_tokens SET revoked_at = COALESCE(revoked_at, ?)
		WHERE id = ? AND issuer = ? AND subject = ?
	`, time.Now().UTC(), id, strings.TrimSpace(issuer), strings.TrimSpace(subject))
	if err != nil {
		return fmt.Errorf("revoke api token: %w", err)
	}
	count, err := result.RowsAffected()
	if err != nil {
		return fmt.Errorf("revoke api token result: %w", err)
	}
	if count == 0 {
		return ErrAPITokenNotFound
	}
	return nil
}

// RevokePersonalAPIToken revokes only a token owned by the authenticated
// session.
func (s *SessionStore) RevokePersonalAPIToken(ctx context.Context, sess *Session, id string) error {
	if sess == nil {
		return errors.New("authenticated session is required")
	}
	return s.RevokeAPIToken(ctx, sess.Issuer, sess.Subject, id)
}

// VerifyAPIToken verifies an opaque personal token and records its use. The
// lookup first uses the public id, then compares HMAC values in constant time.
func (s *SessionStore) VerifyAPIToken(ctx context.Context, plaintext string) (*Session, error) {
	publicID := apiTokenPublicID(plaintext)
	if publicID == "" || len(s.secret) == 0 {
		return nil, ErrAPITokenNotFound
	}
	var id, name, issuer, subject, scopesJSON, storedHash string
	var createdAt, expiresAt time.Time
	err := s.db.QueryRowContext(ctx, `
		SELECT id, name, issuer, subject, scopes, token_hash, created_at, expires_at
		FROM api_tokens WHERE id = ?
	`, publicID).Scan(&id, &name, &issuer, &subject, &scopesJSON, &storedHash, &createdAt, &expiresAt)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrAPITokenNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("select api token: %w", err)
	}
	expected := s.apiTokenHash(plaintext)
	if !hmac.Equal([]byte(storedHash), []byte(expected)) {
		return nil, ErrAPITokenNotFound
	}
	now := time.Now().UTC()
	if !now.Before(expiresAt) {
		return nil, ErrAPITokenExpired
	}
	var revokedAt sql.NullTime
	if err := s.db.QueryRowContext(ctx, `SELECT revoked_at FROM api_tokens WHERE id = ?`, id).Scan(&revokedAt); err != nil {
		return nil, fmt.Errorf("check api token revocation: %w", err)
	}
	if revokedAt.Valid {
		return nil, ErrAPITokenRevoked
	}
	scopes, err := unmarshalScopes(scopesJSON)
	if err != nil {
		return nil, fmt.Errorf("decode api token scopes: %w", err)
	}
	result, err := s.db.ExecContext(ctx, `UPDATE api_tokens SET last_used_at = ? WHERE id = ? AND revoked_at IS NULL`, now, id)
	if err != nil {
		return nil, fmt.Errorf("record api token use: %w", err)
	}
	if changed, err := result.RowsAffected(); err != nil {
		return nil, fmt.Errorf("record api token use result: %w", err)
	} else if changed == 0 {
		return nil, ErrAPITokenRevoked
	}
	return &Session{
		ID: "api-token:" + id, Name: name, Email: subject,
		Issuer: issuer, Subject: subject, Scopes: scopes,
		CreatedAt: createdAt, ExpiresAt: expiresAt,
		AuthMethod: APITokenAuthMethod,
	}, nil
}

// APITokenVerifier adapts the persistent verifier to BearerTokenVerifier.
type APITokenVerifier struct{ store *SessionStore }

func NewAPITokenVerifier(store *SessionStore) *APITokenVerifier {
	return &APITokenVerifier{store: store}
}

func (v *APITokenVerifier) Verify(token string) (*Session, error) {
	if v == nil || v.store == nil {
		return nil, ErrAPITokenNotFound
	}
	return v.store.VerifyAPIToken(context.Background(), token)
}

func (v *APITokenVerifier) VerifyWithRequest(r *http.Request, token string) (*Session, error) {
	if v == nil || v.store == nil {
		return nil, ErrAPITokenNotFound
	}
	return v.store.VerifyAPIToken(r.Context(), token)
}

func (s *SessionStore) apiTokenHash(token string) string {
	h := hmac.New(sha256.New, s.secret)
	_, _ = h.Write([]byte(token))
	return hex.EncodeToString(h.Sum(nil))
}

func randomTokenPublicID() (string, error) {
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		return "", fmt.Errorf("generate token id: %w", err)
	}
	return apiTokenPrefix + hex.EncodeToString(b), nil
}

func apiTokenPublicID(token string) string {
	token = strings.TrimSpace(token)
	if strings.HasPrefix(token, apiTokenPrefix) && !strings.Contains(token, ".") {
		return token
	}
	parts := strings.SplitN(token, ".", 2)
	if len(parts) != 2 || !strings.HasPrefix(parts[0], apiTokenPrefix) || len(parts[1]) < 43 {
		return ""
	}
	return parts[0]
}

func validateAPITokenScopes(scopes []string) ([]string, error) {
	result := normalizeScopes(scopes)
	if len(result) == 0 {
		return nil, errors.New("at least one token scope is required")
	}
	for _, scope := range result {
		if scope != "cxdb:read" && scope != "cxdb:write" {
			return nil, fmt.Errorf("invalid token scope %q", scope)
		}
	}
	return result, nil
}

func normalizeScopes(scopes []string) []string {
	seen := make(map[string]bool, len(scopes))
	result := make([]string, 0, len(scopes))
	for _, scope := range scopes {
		scope = strings.TrimSpace(scope)
		if scope != "" && !seen[scope] {
			seen[scope] = true
			result = append(result, scope)
		}
	}
	return result
}

func marshalScopes(scopes []string) (string, error) {
	// JSON is intentionally used rather than a delimiter, so scope values can
	// be extended in future migrations without ambiguous parsing.
	data, err := json.Marshal(normalizeScopes(scopes))
	return string(data), err
}

func unmarshalScopes(raw string) ([]string, error) {
	var scopes []string
	if err := json.Unmarshal([]byte(raw), &scopes); err != nil {
		return nil, err
	}
	return normalizeScopes(scopes), nil
}
