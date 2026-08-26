// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package config

import (
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/joho/godotenv"
)

// Config captures all runtime configuration for the gateway.
// Values are sourced from environment variables so they can
// be injected locally via a .env file or via platform secrets.
type Config struct {
	GoogleClientID      string
	GoogleClientSecret  string
	GoogleAllowedDomain string
	OIDCEnabled         bool
	OIDCIssuerURL       string
	OIDCClientID        string
	OIDCClientSecret    string
	OIDCAllowedDomains  []string

	PublicBaseURL      string
	PublicAllowedHosts []string
	TrustedProxyCIDRs  []string

	SessionSecret string
	DatabasePath  string
	SessionTTL    time.Duration

	Port         string
	CookieName   string
	CookieDomain string

	// Backend configuration
	CXDBBackendURL string

	// DevMode relaxes auth in local development by allowing the gateway
	// to inject a synthetic session when no cookie is present. It is
	// only enabled when DEV_MODE=true and PUBLIC_BASE_URL points at
	// localhost. Never enable this in production.
	DevMode bool

	// K8s OIDC authentication for in-cluster service accounts
	K8sOIDCEnabled           bool
	K8sOIDCIssuerURL         string
	K8sOIDCAudience          string
	K8sOIDCAllowedNamespaces []string

	// AWS IAM authentication via token exchange
	AWSIAMEnabled      bool
	AWSIAMAllowedRoles []string // ARN patterns with wildcards
	AWSRegion          string
	AWSIAMTokenTTL     time.Duration

	// Renderer CSP configuration
	// List of allowed origins for loading external renderer ESM modules
	AllowedRendererOrigins []string
}

const (
	defaultPort            = "8080"
	defaultCookieName      = "cxdb_session"
	defaultSessionTTL      = 24 * time.Hour
	defaultBaseURL         = "http://localhost:8080"
	defaultDBPath          = "./data/sessions.db"
	defaultCXDBBackendURL  = "http://127.0.0.1:9010"
	defaultAWSIAMTokenTTL  = 1 * time.Hour
	defaultK8sOIDCAudience = "cxdb.local"
)

// Load reads configuration from environment variables and validates
// required fields. Missing required settings are returned as an error
// so startup fails fast rather than producing confusing runtime errors.
func Load() (Config, error) {
	// Best-effort load from common .env locations so `make run` and
	// direct `go run` inside subdirs both work without manual `source`.
	_ = godotenv.Load(".env", "../.env", "../../.env")

	cfg := Config{
		GoogleClientID:      strings.TrimSpace(os.Getenv("GOOGLE_CLIENT_ID")),
		GoogleClientSecret:  strings.TrimSpace(os.Getenv("GOOGLE_CLIENT_SECRET")),
		PublicBaseURL:       firstNonEmpty(os.Getenv("PUBLIC_BASE_URL"), defaultBaseURL),
		PublicAllowedHosts:  splitAndTrim(firstNonEmpty(os.Getenv("PUBLIC_ALLOWED_HOSTS"), "")),
		TrustedProxyCIDRs:   splitAndTrim(firstNonEmpty(os.Getenv("TRUSTED_PROXY_CIDRS"), "")),
		SessionSecret:       strings.TrimSpace(os.Getenv("SESSION_SECRET")),
		DatabasePath:        firstNonEmpty(os.Getenv("DATABASE_PATH"), defaultDBPath),
		Port:                firstNonEmpty(os.Getenv("PORT"), defaultPort),
		CookieName:          firstNonEmpty(os.Getenv("SESSION_COOKIE_NAME"), defaultCookieName),
		CookieDomain:        strings.TrimSpace(os.Getenv("SESSION_COOKIE_DOMAIN")),
		GoogleAllowedDomain: strings.ToLower(strings.TrimSpace(os.Getenv("GOOGLE_ALLOWED_DOMAIN"))),
		SessionTTL:          defaultSessionTTL,
		CXDBBackendURL:      firstNonEmpty(os.Getenv("CXDB_BACKEND_URL"), defaultCXDBBackendURL),
	}
	cfg.OIDCEnabled = parseBoolEnv("OIDC_ENABLED")
	cfg.OIDCIssuerURL = strings.TrimSuffix(strings.TrimSpace(os.Getenv("OIDC_ISSUER_URL")), "/")
	cfg.OIDCClientID = strings.TrimSpace(os.Getenv("OIDC_CLIENT_ID"))
	cfg.OIDCClientSecret = strings.TrimSpace(os.Getenv("OIDC_CLIENT_SECRET"))
	cfg.OIDCAllowedDomains = splitAndTrim(os.Getenv("OIDC_ALLOWED_DOMAINS"))

	if ttlStr := strings.TrimSpace(os.Getenv("SESSION_TTL_HOURS")); ttlStr != "" {
		if hours, err := strconv.Atoi(ttlStr); err == nil && hours > 0 {
			cfg.SessionTTL = time.Duration(hours) * time.Hour
		} else {
			return Config{}, fmt.Errorf("invalid SESSION_TTL_HOURS: %w", err)
		}
	}

	if len(cfg.PublicAllowedHosts) == 0 {
		if host := hostnameFromURL(cfg.PublicBaseURL); host != "" {
			cfg.PublicAllowedHosts = []string{host}
		}
	}

	// Enable dev-mode auth bypass only when explicitly requested and
	// when the public URL clearly points at a localhost instance.
	if parseBoolEnv("DEV_MODE") && isLocalhostURL(cfg.PublicBaseURL) {
		cfg.DevMode = true
	}

	// K8s OIDC configuration
	cfg.K8sOIDCEnabled = parseBoolEnv("K8S_OIDC_ENABLED")
	cfg.K8sOIDCIssuerURL = strings.TrimSpace(os.Getenv("K8S_OIDC_ISSUER_URL"))
	cfg.K8sOIDCAudience = firstNonEmpty(os.Getenv("K8S_OIDC_AUDIENCE"), defaultK8sOIDCAudience)
	cfg.K8sOIDCAllowedNamespaces = splitAndTrim(os.Getenv("K8S_OIDC_ALLOWED_NAMESPACES"))

	// AWS IAM configuration
	cfg.AWSIAMEnabled = parseBoolEnv("AWS_IAM_ENABLED")
	cfg.AWSIAMAllowedRoles = splitAndTrimPreserveCase(os.Getenv("AWS_IAM_ALLOWED_ROLES"))
	cfg.AWSRegion = firstNonEmpty(os.Getenv("AWS_REGION"), "us-west-2")
	cfg.AWSIAMTokenTTL = defaultAWSIAMTokenTTL
	if ttlStr := strings.TrimSpace(os.Getenv("AWS_IAM_TOKEN_TTL")); ttlStr != "" {
		if d, err := time.ParseDuration(ttlStr); err == nil && d > 0 {
			cfg.AWSIAMTokenTTL = d
		}
	}

	// Renderer origin allowlist for CSP script-src directive
	// Defaults to common public CDNs if not specified
	cfg.AllowedRendererOrigins = splitAndTrimPreserveCase(os.Getenv("ALLOWED_RENDERER_ORIGINS"))
	if len(cfg.AllowedRendererOrigins) == 0 {
		cfg.AllowedRendererOrigins = []string{
			"https://esm.sh",
			"https://cdn.jsdelivr.net",
			"https://unpkg.com",
		}
	}

	if err := cfg.validate(); err != nil {
		return Config{}, err
	}

	if abs, err := filepath.Abs(cfg.DatabasePath); err == nil {
		cfg.DatabasePath = abs
	}
	return cfg, nil
}

func (c Config) validate() error {
	var missing []string
	if len(strings.TrimSpace(c.SessionSecret)) < 32 {
		missing = append(missing, "SESSION_SECRET (minimum 32 bytes)")
	}
	googleConfigured := c.GoogleClientID != "" || c.GoogleClientSecret != "" || c.GoogleAllowedDomain != ""
	if googleConfigured {
		if c.GoogleClientID == "" {
			missing = append(missing, "GOOGLE_CLIENT_ID")
		}
		if c.GoogleClientSecret == "" {
			missing = append(missing, "GOOGLE_CLIENT_SECRET")
		}
		if c.GoogleAllowedDomain == "" {
			missing = append(missing, "GOOGLE_ALLOWED_DOMAIN")
		}
	}
	if c.OIDCEnabled {
		if c.OIDCIssuerURL == "" {
			missing = append(missing, "OIDC_ISSUER_URL")
		}
		if c.OIDCClientID == "" {
			missing = append(missing, "OIDC_CLIENT_ID")
		}
		if c.OIDCClientSecret == "" {
			missing = append(missing, "OIDC_CLIENT_SECRET")
		}
		if len(c.OIDCAllowedDomains) == 0 {
			missing = append(missing, "OIDC_ALLOWED_DOMAINS")
		}
	}
	if !googleConfigured && !c.OIDCEnabled && !c.DevMode {
		missing = append(missing, "GOOGLE_CLIENT_ID or OIDC_ENABLED=true")
	}

	// Conditional validation for K8s OIDC
	if c.K8sOIDCEnabled {
		if c.K8sOIDCIssuerURL == "" {
			missing = append(missing, "K8S_OIDC_ISSUER_URL (required when K8S_OIDC_ENABLED=true)")
		}
	}

	// Conditional validation for AWS IAM
	if c.AWSIAMEnabled {
		if len(c.AWSIAMAllowedRoles) == 0 {
			missing = append(missing, "AWS_IAM_ALLOWED_ROLES (required when AWS_IAM_ENABLED=true)")
		}
	}

	if len(missing) > 0 {
		return fmt.Errorf("missing required env vars: %s", strings.Join(missing, ", "))
	}
	if _, err := url.Parse(c.CXDBBackendURL); err != nil {
		return errors.New("invalid CXDB_BACKEND_URL")
	}
	for _, cidr := range c.TrustedProxyCIDRs {
		if _, _, err := net.ParseCIDR(cidr); err != nil {
			return fmt.Errorf("invalid TRUSTED_PROXY_CIDRS entry %q", cidr)
		}
	}
	return nil
}

func splitAndTrim(raw string) []string {
	parts := strings.Split(raw, ",")
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if v := strings.ToLower(strings.TrimSpace(p)); v != "" {
			out = append(out, v)
		}
	}
	return out
}

// splitAndTrimPreserveCase splits on comma and trims whitespace but preserves case.
// Use for case-sensitive values like ARNs.
func splitAndTrimPreserveCase(raw string) []string {
	parts := strings.Split(raw, ",")
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if v := strings.TrimSpace(p); v != "" {
			out = append(out, v)
		}
	}
	return out
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if strings.TrimSpace(v) != "" {
			return v
		}
	}
	return ""
}

func parseBoolEnv(key string) bool {
	raw := strings.TrimSpace(os.Getenv(key))
	if raw == "" {
		return false
	}
	b, err := strconv.ParseBool(raw)
	if err != nil {
		return false
	}
	return b
}

func isLocalhostURL(raw string) bool {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.User != nil || u.Host == "" {
		return false
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return false
	}
	switch strings.ToLower(u.Hostname()) {
	case "localhost", "127.0.0.1", "::1":
		return true
	default:
		return false
	}
}

func hostnameFromURL(raw string) string {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil {
		return ""
	}
	return strings.ToLower(strings.TrimSpace(u.Hostname()))
}
