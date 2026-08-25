// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/coreos/go-oidc/v3/oidc"
	"golang.org/x/oauth2"
)

const browserOIDCTransactionCookie = "cxdb_oidc_transaction"

// BrowserOIDC implements a verified OIDC authorization-code flow for browser users.
type BrowserOIDC struct {
	issuer         string
	allowedDomains map[string]struct{}
	config         oauth2.Config
	verifier       *oidc.IDTokenVerifier
	sessions       *SessionStore
}

type browserOIDCTransaction struct {
	State        string `json:"state"`
	Nonce        string `json:"nonce"`
	CodeVerifier string `json:"code_verifier"`
	ReturnTo     string `json:"return_to,omitempty"`
	ExpiresAt    int64  `json:"expires_at"`
}

// NewBrowserOIDC performs OIDC discovery and pins token verification to issuer and client ID.
func NewBrowserOIDC(ctx context.Context, issuer, clientID, clientSecret, publicBaseURL string, allowedDomains []string, sessions *SessionStore) (*BrowserOIDC, error) {
	issuer = strings.TrimSuffix(strings.TrimSpace(issuer), "/")
	issuerURL, err := url.Parse(issuer)
	if err != nil || issuerURL.Scheme != "https" || issuerURL.Host == "" || issuerURL.User != nil {
		return nil, errors.New("OIDC issuer must be an HTTPS origin")
	}
	provider, err := oidc.NewProvider(ctx, issuer)
	if err != nil {
		return nil, fmt.Errorf("discover OIDC provider: %w", err)
	}
	domains := make(map[string]struct{}, len(allowedDomains))
	for _, domain := range allowedDomains {
		domain = strings.ToLower(strings.TrimSpace(strings.TrimPrefix(domain, "@")))
		if domain != "" {
			domains[domain] = struct{}{}
		}
	}
	if len(domains) == 0 {
		return nil, errors.New("at least one OIDC email domain is required")
	}
	return &BrowserOIDC{
		issuer:         issuer,
		allowedDomains: domains,
		config: oauth2.Config{
			ClientID:     clientID,
			ClientSecret: clientSecret,
			Endpoint:     provider.Endpoint(),
			RedirectURL:  strings.TrimSuffix(publicBaseURL, "/") + "/auth/oidc/callback",
			Scopes:       []string{oidc.ScopeOpenID, "profile", "email"},
		},
		verifier: provider.Verifier(&oidc.Config{ClientID: clientID}),
		sessions: sessions,
	}, nil
}

// LoginHandler starts the OIDC flow with state, nonce, and PKCE S256.
func (o *BrowserOIDC) LoginHandler(w http.ResponseWriter, r *http.Request) {
	state, err := randomState()
	if err != nil {
		http.Error(w, "unable to start login", http.StatusInternalServerError)
		return
	}
	nonce, err := randomState()
	if err != nil {
		http.Error(w, "unable to start login", http.StatusInternalServerError)
		return
	}
	verifier := oauth2.GenerateVerifier()
	tx := browserOIDCTransaction{
		State:        state,
		Nonce:        nonce,
		CodeVerifier: verifier,
		ReturnTo:     safeLocalReturn(r.URL.Query().Get("return_to")),
		ExpiresAt:    time.Now().Add(10 * time.Minute).Unix(),
	}
	encoded, err := json.Marshal(tx)
	if err != nil {
		http.Error(w, "unable to start login", http.StatusInternalServerError)
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name:     browserOIDCTransactionCookie,
		Value:    o.sessions.sign(base64.RawURLEncoding.EncodeToString(encoded)),
		Path:     "/",
		MaxAge:   600,
		HttpOnly: true,
		Secure:   o.sessions.Secure(),
		SameSite: http.SameSiteLaxMode,
	})
	authURL := o.config.AuthCodeURL(state,
		oauth2.AccessTypeOnline,
		oauth2.S256ChallengeOption(verifier),
		oauth2.SetAuthURLParam("nonce", nonce),
	)
	http.Redirect(w, r, authURL, http.StatusFound)
}

// CallbackHandler verifies the authorization response and creates a browser session.
func (o *BrowserOIDC) CallbackHandler(w http.ResponseWriter, r *http.Request) {
	tx, err := o.transaction(r)
	o.clearTransaction(w)
	if err != nil || !subtleEqual(tx.State, r.URL.Query().Get("state")) {
		http.Redirect(w, r, "/login?error=state", http.StatusFound)
		return
	}
	if r.URL.Query().Get("error") != "" {
		http.Redirect(w, r, "/login?error=access_denied", http.StatusFound)
		return
	}
	token, err := o.config.Exchange(r.Context(), r.URL.Query().Get("code"), oauth2.VerifierOption(tx.CodeVerifier))
	if err != nil {
		http.Redirect(w, r, "/login?error=exchange", http.StatusFound)
		return
	}
	rawIDToken, ok := token.Extra("id_token").(string)
	if !ok || rawIDToken == "" {
		http.Redirect(w, r, "/login?error=id_token", http.StatusFound)
		return
	}
	idToken, err := o.verifier.Verify(r.Context(), rawIDToken)
	if err != nil {
		http.Redirect(w, r, "/login?error=id_token", http.StatusFound)
		return
	}
	var claims struct {
		Subject       string `json:"sub"`
		Nonce         string `json:"nonce"`
		Email         string `json:"email"`
		EmailVerified bool   `json:"email_verified"`
		Name          string `json:"name"`
		Picture       string `json:"picture"`
	}
	if err := idToken.Claims(&claims); err != nil || claims.Subject == "" || claims.Email == "" || !claims.EmailVerified || !subtleEqual(claims.Nonce, tx.Nonce) || !o.emailAllowed(claims.Email) {
		http.Redirect(w, r, "/login?error=unauthorized", http.StatusFound)
		return
	}
	if claims.Name == "" {
		claims.Name = claims.Email
	}
	sessionID, err := o.sessions.CreateForIdentity(r.Context(), o.issuer, claims.Subject, strings.ToLower(claims.Email), claims.Name, claims.Picture, "oidc", []string{"cxdb:read", "cxdb:write"})
	if err != nil {
		http.Error(w, "unable to create session", http.StatusInternalServerError)
		return
	}
	o.sessions.SetCookie(w, sessionID)
	destination := tx.ReturnTo
	if destination == "" {
		destination = "/"
	}
	http.Redirect(w, r, destination, http.StatusFound)
}

func (o *BrowserOIDC) transaction(r *http.Request) (browserOIDCTransaction, error) {
	var tx browserOIDCTransaction
	cookie, err := r.Cookie(browserOIDCTransactionCookie)
	if err != nil {
		return tx, err
	}
	value, ok := o.sessions.verify(cookie.Value)
	if !ok {
		return tx, errors.New("invalid transaction signature")
	}
	encoded, err := base64.RawURLEncoding.DecodeString(value)
	if err != nil {
		return tx, err
	}
	if err := json.Unmarshal(encoded, &tx); err != nil {
		return tx, err
	}
	if time.Now().Unix() > tx.ExpiresAt || tx.State == "" || tx.Nonce == "" || tx.CodeVerifier == "" {
		return tx, errors.New("expired OIDC transaction")
	}
	return tx, nil
}

func (o *BrowserOIDC) clearTransaction(w http.ResponseWriter) {
	http.SetCookie(w, &http.Cookie{Name: browserOIDCTransactionCookie, Path: "/", MaxAge: -1, HttpOnly: true, Secure: o.sessions.Secure(), SameSite: http.SameSiteLaxMode})
}

func (o *BrowserOIDC) emailAllowed(email string) bool {
	_, domain, ok := strings.Cut(strings.ToLower(strings.TrimSpace(email)), "@")
	if !ok || domain == "" {
		return false
	}
	_, ok = o.allowedDomains[domain]
	return ok
}

func safeLocalReturn(raw string) string {
	if raw == "" || !strings.HasPrefix(raw, "/") || strings.HasPrefix(raw, "//") || strings.Contains(raw, "\\") || strings.Contains(strings.ToLower(raw), "%5c") {
		return ""
	}
	u, err := url.Parse(raw)
	if err != nil || u.IsAbs() || u.Host != "" {
		return ""
	}
	// Browsers can decode escaped separators before navigation. Reject paths
	// that become an external authority or contain a decoded backslash.
	if strings.Contains(u.Path, "\\") || strings.HasPrefix(u.Path, "//") {
		return ""
	}
	return u.RequestURI()
}
