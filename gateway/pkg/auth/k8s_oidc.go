// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/lestrrat-go/jwx/v2/jwk"
	"github.com/lestrrat-go/jwx/v2/jwt"
)

// K8sOIDCVerifier validates Kubernetes service account tokens using OIDC.
// These tokens are JWTs signed by the cluster's OIDC provider and can be
// verified by fetching the JWKS from the issuer's discovery endpoint.
type K8sOIDCVerifier struct {
	issuerURL         string
	audience          string
	allowedNamespaces map[string]bool
	keySet            jwk.Set
	keySetMu          sync.RWMutex
	lastRefresh       time.Time
	refreshInterval   time.Duration
	debug             bool
}

// NewK8sOIDCVerifier creates a new verifier for K8s service account tokens.
func NewK8sOIDCVerifier(issuerURL, audience string, allowedNamespaces []string) (*K8sOIDCVerifier, error) {
	v := &K8sOIDCVerifier{
		issuerURL:         strings.TrimSuffix(issuerURL, "/"),
		audience:          audience,
		allowedNamespaces: make(map[string]bool),
		refreshInterval:   1 * time.Hour,
		debug:             strings.Contains(os.Getenv("DEBUG"), "auth") || strings.Contains(os.Getenv("DEBUG"), "all"),
	}

	for _, ns := range allowedNamespaces {
		v.allowedNamespaces[ns] = true
	}

	// Initial JWKS fetch
	if err := v.refreshKeySet(context.Background()); err != nil {
		return nil, fmt.Errorf("fetch JWKS: %w", err)
	}

	return v, nil
}

// Verify validates a K8s service account token and returns a Session if valid.
func (v *K8sOIDCVerifier) Verify(tokenString string) (*Session, error) {
	// Refresh JWKS if stale
	v.keySetMu.RLock()
	needsRefresh := time.Since(v.lastRefresh) > v.refreshInterval
	v.keySetMu.RUnlock()

	if needsRefresh {
		if err := v.refreshKeySet(context.Background()); err != nil {
			if v.debug {
				log.Printf("[k8s-oidc] JWKS refresh failed: %v", err)
			}
			// Continue with existing keys if refresh fails
		}
	}

	v.keySetMu.RLock()
	keySet := v.keySet
	v.keySetMu.RUnlock()

	// Parse and validate the JWT
	token, err := jwt.Parse([]byte(tokenString),
		jwt.WithKeySet(keySet),
		jwt.WithValidate(true),
		jwt.WithIssuer(v.issuerURL),
		jwt.WithAudience(v.audience),
	)
	if err != nil {
		if v.debug {
			log.Printf("[k8s-oidc] token validation failed: %v", err)
		}
		return nil, fmt.Errorf("invalid token: %w", err)
	}

	// Extract subject: system:serviceaccount:<namespace>:<name>
	sub := token.Subject()
	namespace, saName, err := parseK8sSubject(sub)
	if err != nil {
		if v.debug {
			log.Printf("[k8s-oidc] invalid subject format: %s", sub)
		}
		return nil, fmt.Errorf("invalid subject: %w", err)
	}

	// Check namespace allowlist (if configured)
	if len(v.allowedNamespaces) > 0 && !v.allowedNamespaces[namespace] {
		if v.debug {
			log.Printf("[k8s-oidc] namespace %s not in allowlist", namespace)
		}
		return nil, fmt.Errorf("namespace %s not allowed", namespace)
	}

	if v.debug {
		log.Printf("[k8s-oidc] authenticated: %s/%s", namespace, saName)
	}

	return &Session{
		ID:        fmt.Sprintf("k8s:%s:%s", namespace, saName),
		Email:     fmt.Sprintf("%s/%s@k8s.local", namespace, saName),
		Name:      fmt.Sprintf("ServiceAccount: %s/%s", namespace, saName),
		CreatedAt: token.IssuedAt(),
		ExpiresAt: token.Expiration(),
	}, nil
}

// refreshKeySet fetches the JWKS from the OIDC discovery endpoint.
func (v *K8sOIDCVerifier) refreshKeySet(ctx context.Context) error {
	// Fetch OIDC discovery document
	discoveryURL := v.issuerURL + "/.well-known/openid-configuration"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, discoveryURL, nil)
	if err != nil {
		return err
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("fetch discovery: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("discovery returned %d", resp.StatusCode)
	}

	var discovery struct {
		JWKSURI string `json:"jwks_uri"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&discovery); err != nil {
		return fmt.Errorf("decode discovery: %w", err)
	}

	if discovery.JWKSURI == "" {
		return fmt.Errorf("no jwks_uri in discovery document")
	}

	// Fetch JWKS
	keySet, err := jwk.Fetch(ctx, discovery.JWKSURI)
	if err != nil {
		return fmt.Errorf("fetch JWKS: %w", err)
	}

	v.keySetMu.Lock()
	v.keySet = keySet
	v.lastRefresh = time.Now()
	v.keySetMu.Unlock()

	if v.debug {
		log.Printf("[k8s-oidc] refreshed JWKS from %s", discovery.JWKSURI)
	}

	return nil
}

// parseK8sSubject extracts namespace and service account name from a K8s subject.
// Format: system:serviceaccount:<namespace>:<name>
func parseK8sSubject(sub string) (namespace, name string, err error) {
	parts := strings.Split(sub, ":")
	if len(parts) != 4 || parts[0] != "system" || parts[1] != "serviceaccount" {
		return "", "", fmt.Errorf("expected system:serviceaccount:<namespace>:<name>, got %s", sub)
	}
	return parts[2], parts[3], nil
}
