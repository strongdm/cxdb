// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"database/sql"
	"errors"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func testAPITokenStore(t *testing.T) *SessionStore {
	t.Helper()
	store, err := NewSessionStore(filepath.Join(t.TempDir(), "auth.sqlite"), "session", time.Hour, "", false, "test-hmac-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	return store
}

func TestAPITokenCreateReturnsPlaintextOnceAndStoresOnlyHash(t *testing.T) {
	store := testAPITokenStore(t)
	meta, plaintext, err := store.CreateAPIToken(context.Background(), APITokenCreateRequest{
		Name: "laptop", Issuer: "https://issuer.example", Subject: "user-1",
		Scopes: []string{"cxdb:write", "cxdb:read"}, ExpiresAt: time.Now().Add(time.Hour),
	})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(plaintext, "cxpat_") || len(strings.SplitN(plaintext, ".", 2)[1]) < 43 {
		t.Fatalf("token does not have the required opaque format: %q", plaintext)
	}
	var hash string
	if err := store.db.QueryRow(`SELECT token_hash FROM api_tokens WHERE id = ?`, meta.ID).Scan(&hash); err != nil {
		t.Fatal(err)
	}
	if hash == plaintext || hash == "" || len(hash) != 64 {
		t.Fatalf("database contains token plaintext or invalid hash")
	}
	var plaintextColumn string
	err = store.db.QueryRow(`SELECT COALESCE(token, '') FROM api_tokens WHERE id = ?`, meta.ID).Scan(&plaintextColumn)
	if err == nil {
		t.Fatalf("api_tokens unexpectedly has a plaintext token column")
	}
}

func TestAPITokenOwnershipExpiryRevocationLastUseAndScopes(t *testing.T) {
	store := testAPITokenStore(t)
	ctx := context.Background()
	meta, plaintext, err := store.CreateAPIToken(ctx, APITokenCreateRequest{
		Name: "ci", Issuer: "issuer", Subject: "alice", Scopes: []string{"cxdb:read"}, ExpiresAt: time.Now().Add(time.Hour),
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.ListAPITokens(ctx, "issuer", "bob"); err != nil {
		t.Fatal(err)
	}
	if err := store.RevokeAPIToken(ctx, "issuer", "bob", meta.ID); !errors.Is(err, ErrAPITokenNotFound) {
		t.Fatalf("wrong-owner revoke error = %v", err)
	}
	sess, err := store.VerifyAPIToken(ctx, plaintext)
	if err != nil {
		t.Fatal(err)
	}
	if sess.Subject != "alice" || sess.Issuer != "issuer" || !sess.HasScope("cxdb:read") || sess.IsAPIToken() == false {
		t.Fatalf("verified identity/scopes not preserved: %+v", sess)
	}
	var used sql.NullTime
	if err := store.db.QueryRow(`SELECT last_used_at FROM api_tokens WHERE id = ?`, meta.ID).Scan(&used); err != nil {
		t.Fatal(err)
	}
	if !used.Valid {
		t.Fatal("last_used_at was not recorded")
	}
	if err := store.RevokeAPIToken(ctx, "issuer", "alice", meta.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := store.VerifyAPIToken(ctx, plaintext); !errors.Is(err, ErrAPITokenRevoked) {
		t.Fatal("revoked token was accepted")
	}

	_, expired, err := store.CreateAPIToken(ctx, APITokenCreateRequest{
		Name: "old", Issuer: "issuer", Subject: "alice", Scopes: []string{"cxdb:write"}, ExpiresAt: time.Now().Add(-time.Minute),
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.VerifyAPIToken(ctx, expired); err == nil || !strings.Contains(err.Error(), "expired") {
		t.Fatalf("expired token error = %v", err)
	}
	if _, _, err := store.CreateAPIToken(ctx, APITokenCreateRequest{Name: "bad", Issuer: "issuer", Subject: "alice", Scopes: []string{"admin"}}); err == nil {
		t.Fatal("invalid scope accepted")
	}
}
