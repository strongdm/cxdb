// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"

	"golang.org/x/oauth2"
	"golang.org/x/oauth2/google"
)

// GoogleAuth wires Google OAuth2 handlers with the session store.
type GoogleAuth struct {
	cfg           *oauth2.Config
	stateMaxAge   time.Duration
	allowedDomain string
	allowedHosts  map[string]bool
	sessions      *SessionStore
	publicURL     string
}

func NewGoogleAuth(publicBaseURL string, clientID, clientSecret string, allowedDomain string, allowedHosts []string, sessions *SessionStore) *GoogleAuth {
	stateAge := 10 * time.Minute
	hostMap := make(map[string]bool, len(allowedHosts))
	for _, h := range allowedHosts {
		if v := strings.ToLower(strings.TrimSpace(h)); v != "" {
			hostMap[v] = true
		}
	}
	redirectURL := strings.TrimSuffix(publicBaseURL, "/") + "/auth/google/callback"
	return &GoogleAuth{
		cfg: &oauth2.Config{
			ClientID:     clientID,
			ClientSecret: clientSecret,
			RedirectURL:  redirectURL,
			Scopes: []string{
				"https://www.googleapis.com/auth/userinfo.email",
				"https://www.googleapis.com/auth/userinfo.profile",
			},
			Endpoint: google.Endpoint,
		},
		stateMaxAge:   stateAge,
		allowedDomain: strings.ToLower(strings.TrimSpace(allowedDomain)),
		allowedHosts:  hostMap,
		sessions:      sessions,
		publicURL:     publicBaseURL,
	}
}

// LoginHandler redirects users to Google's consent screen.
func (g *GoogleAuth) LoginHandler(w http.ResponseWriter, r *http.Request) {
	state, err := randomState()
	if err != nil {
		http.Error(w, "unable to create state", http.StatusInternalServerError)
		return
	}
	g.setPostAuthRedirectCookie(w, r)
	http.SetCookie(w, &http.Cookie{
		Name:     "oauth_state",
		Value:    state,
		Domain:   g.sessions.Domain(),
		Path:     "/",
		MaxAge:   int(g.stateMaxAge.Seconds()),
		HttpOnly: true,
		Secure:   g.sessions.Secure(),
		SameSite: http.SameSiteLaxMode,
	})
	authURL := g.cfg.AuthCodeURL(state, oauth2.AccessTypeOnline)
	http.Redirect(w, r, authURL, http.StatusFound)
}

// CallbackHandler exchanges the auth code, checks the email allowlist,
// issues a session, and redirects to the homepage.
func (g *GoogleAuth) CallbackHandler(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	state := r.URL.Query().Get("state")
	code := r.URL.Query().Get("code")
	if errParam := r.URL.Query().Get("error"); errParam != "" {
		http.Redirect(w, r, "/login?error=access_denied", http.StatusFound)
		return
	}

	if !g.validState(r, state) {
		http.Redirect(w, r, "/login?error=state", http.StatusFound)
		return
	}

	token, err := g.cfg.Exchange(ctx, code)
	if err != nil {
		if g.sessions.Debug() {
			log.Printf("[auth] exchange error: %v", err)
		}
		http.Redirect(w, r, "/login?error=exchange", http.StatusFound)
		return
	}

	user, err := g.fetchUser(ctx, token)
	if err != nil {
		if g.sessions.Debug() {
			log.Printf("[auth] userinfo error: %v", err)
		}
		http.Redirect(w, r, "/login?error=profile", http.StatusFound)
		return
	}
	email := strings.ToLower(user.Email)

	// Check email domain - ALWAYS enforced, deny if not from allowed domain
	if !strings.HasSuffix(email, "@"+g.allowedDomain) {
		if g.sessions.Debug() {
			log.Printf("[auth] unauthorized email %s (not from allowed domain %s)", email, g.allowedDomain)
		}
		http.Redirect(w, r, "/login?error=unauthorized", http.StatusFound)
		return
	}

	name := user.Name
	if name == "" {
		name = email
	}

	sessionID, err := g.sessions.Create(ctx, email, name, user.Picture)
	if err != nil {
		if g.sessions.Debug() {
			log.Printf("[auth] create session error: %v", err)
		}
		http.Error(w, "unable to create session", http.StatusInternalServerError)
		return
	}
	g.sessions.SetCookie(w, sessionID)
	g.clearStateCookie(w)
	if dest := g.postAuthRedirect(w, r); dest != "" {
		http.Redirect(w, r, dest, http.StatusFound)
		return
	}
	http.Redirect(w, r, "/", http.StatusFound)
}

// LogoutHandler clears the session and redirects to login.
func (g *GoogleAuth) LogoutHandler(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	if sess, _ := g.sessions.SessionFromRequest(ctx, r); sess != nil {
		_ = g.sessions.Delete(ctx, sess.ID)
	}
	g.sessions.ClearCookie(w)
	http.Redirect(w, r, "/login", http.StatusFound)
}

type googleUser struct {
	Email   string `json:"email"`
	Name    string `json:"name"`
	Picture string `json:"picture"`
}

func (g *GoogleAuth) fetchUser(ctx context.Context, token *oauth2.Token) (googleUser, error) {
	client := g.cfg.Client(ctx, token)
	resp, err := client.Get("https://www.googleapis.com/oauth2/v2/userinfo")
	if err != nil {
		return googleUser{}, fmt.Errorf("userinfo request: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return googleUser{}, fmt.Errorf("userinfo status: %d", resp.StatusCode)
	}
	var u googleUser
	if err := json.NewDecoder(resp.Body).Decode(&u); err != nil {
		return googleUser{}, fmt.Errorf("decode userinfo: %w", err)
	}
	if u.Email == "" {
		return googleUser{}, errors.New("email missing in profile")
	}
	return u, nil
}

func (g *GoogleAuth) validState(r *http.Request, state string) bool {
	if state == "" {
		return false
	}
	c, err := r.Cookie("oauth_state")
	if err != nil {
		return false
	}
	return subtleEqual(state, c.Value)
}

func randomState() (string, error) {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(b[:]), nil
}

func (g *GoogleAuth) clearStateCookie(w http.ResponseWriter) {
	http.SetCookie(w, &http.Cookie{
		Name:     "oauth_state",
		Value:    "",
		Domain:   g.sessions.Domain(),
		Path:     "/",
		HttpOnly: true,
		Secure:   g.sessions.Secure(),
		SameSite: http.SameSiteLaxMode,
		MaxAge:   -1,
	})
}

func (g *GoogleAuth) setPostAuthRedirectCookie(w http.ResponseWriter, r *http.Request) {
	host := canonicalHost(r)
	if host == "" {
		return
	}
	scheme := "https"
	if forwarded := strings.TrimSpace(r.Header.Get("X-Forwarded-Proto")); forwarded != "" {
		scheme = strings.ToLower(forwarded)
	} else if r.TLS == nil {
		scheme = "http"
	}
	if scheme != "https" && scheme != "http" {
		return
	}
	base := scheme + "://" + host
	if !g.isAllowedRedirectBase(base) {
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name:     "post_auth_redirect",
		Value:    base,
		Domain:   g.sessions.Domain(),
		Path:     "/",
		MaxAge:   int((10 * time.Minute).Seconds()),
		HttpOnly: true,
		Secure:   g.sessions.Secure(),
		SameSite: http.SameSiteLaxMode,
	})
}

func (g *GoogleAuth) postAuthRedirect(w http.ResponseWriter, r *http.Request) string {
	c, err := r.Cookie("post_auth_redirect")
	if err != nil {
		return ""
	}
	g.clearPostAuthRedirectCookie(w)
	base := strings.TrimSpace(c.Value)
	if base == "" {
		return ""
	}
	if !g.isAllowedRedirectBase(base) {
		return ""
	}
	u, err := url.Parse(base)
	if err != nil {
		return ""
	}
	u.Path = "/"
	u.RawQuery = ""
	u.Fragment = ""
	return u.String()
}

func (g *GoogleAuth) clearPostAuthRedirectCookie(w http.ResponseWriter) {
	http.SetCookie(w, &http.Cookie{
		Name:     "post_auth_redirect",
		Value:    "",
		Domain:   g.sessions.Domain(),
		Path:     "/",
		HttpOnly: true,
		Secure:   g.sessions.Secure(),
		SameSite: http.SameSiteLaxMode,
		MaxAge:   -1,
	})
}

func (g *GoogleAuth) isAllowedRedirectBase(rawBaseURL string) bool {
	u, err := url.Parse(rawBaseURL)
	if err != nil {
		return false
	}
	if u.Scheme != "https" && u.Scheme != "http" {
		return false
	}
	if u.User != nil {
		return false
	}
	if u.Path != "" && u.Path != "/" {
		return false
	}
	host := strings.ToLower(u.Hostname())
	if host == "" {
		return false
	}
	if len(g.allowedHosts) > 0 {
		return g.allowedHosts[host]
	}
	domain := strings.ToLower(strings.TrimSpace(strings.TrimPrefix(g.sessions.Domain(), ".")))
	if domain == "" {
		// No cross-subdomain cookies => only allow same-host redirects (but we can't safely know it here).
		return false
	}
	if host == domain || strings.HasSuffix(host, "."+domain) {
		return true
	}
	return false
}

func canonicalHost(r *http.Request) string {
	host := strings.ToLower(strings.TrimSpace(r.Host))
	if host == "" {
		return ""
	}
	if h, _, err := net.SplitHostPort(host); err == nil {
		return h
	}
	return host
}

// AttachUser stores the authenticated session on the request context.
type contextKey string

const userContextKey contextKey = "authedUser"

// WithUser attaches session data to the context for downstream handlers.
func WithUser(ctx context.Context, sess *Session) context.Context {
	return context.WithValue(ctx, userContextKey, sess)
}

// UserFromContext retrieves the session if present.
func UserFromContext(ctx context.Context) *Session {
	val := ctx.Value(userContextKey)
	if sess, ok := val.(*Session); ok {
		return sess
	}
	return nil
}
