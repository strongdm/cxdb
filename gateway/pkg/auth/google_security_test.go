// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"net/http/httptest"
	"path/filepath"
	"testing"
	"time"
)

func TestGooglePostAuthRedirectUsesConfiguredPublicURL(t *testing.T) {
	store, err := NewSessionStore(filepath.Join(t.TempDir(), "sessions.sqlite"), "session", time.Hour, "", true, "test-secret")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = store.Close() }()

	google := &GoogleAuth{
		publicURL:    "https://cxdb.example",
		allowedHosts: map[string]bool{"cxdb.example": true},
		sessions:     store,
	}
	response := httptest.NewRecorder()
	google.setPostAuthRedirectCookie(response)
	cookies := response.Result().Cookies()
	if len(cookies) != 1 || cookies[0].Value != "https://cxdb.example" {
		t.Fatalf("redirect cookie = %+v", cookies)
	}
}
