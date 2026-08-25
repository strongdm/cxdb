// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"path/filepath"
	"testing"
	"time"
)

func TestLegacyBrowserSessionKeepsDefaultIdentityAndScopes(t *testing.T) {
	store, err := NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", false, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	now := time.Now().UTC()
	_, err = store.db.ExecContext(context.Background(), `
		INSERT INTO sessions (id, email, name, picture, created_at, expires_at)
		VALUES (?, ?, ?, ?, ?, ?)
	`, "legacy", "legacy@example.com", "Legacy", "", now, now.Add(time.Hour))
	if err != nil {
		t.Fatal(err)
	}

	session, err := store.Get(context.Background(), "legacy")
	if err != nil {
		t.Fatal(err)
	}
	if session == nil || session.Issuer != "https://accounts.google.com" || session.Subject != "legacy@example.com" || session.AuthMethod != "google_oauth" {
		t.Fatalf("legacy identity = %+v", session)
	}
	if !session.HasScope("cxdb:read") || !session.HasScope("cxdb:write") {
		t.Fatalf("legacy scopes = %v", session.Scopes)
	}
}
