// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/lestrrat-go/jwx/v2/jwa"
	"github.com/lestrrat-go/jwx/v2/jwt"
)

// AWSTokenExchanger handles token exchange for AWS IAM authentication.
// Clients present a presigned STS GetCallerIdentity URL, and receive
// a short-lived CXDB JWT in exchange.
type AWSTokenExchanger struct {
	allowedRolePatterns []*regexp.Regexp
	tokenTTL            time.Duration
	signingKey          []byte
	issuer              string
	audience            string
	debug               bool
	httpClient          *http.Client
}

// NewAWSTokenExchanger creates a new AWS IAM token exchanger.
func NewAWSTokenExchanger(allowedRoles []string, tokenTTL time.Duration, signingKey []byte, issuer string) (*AWSTokenExchanger, error) {
	patterns := make([]*regexp.Regexp, 0, len(allowedRoles))
	for _, role := range allowedRoles {
		// Convert glob pattern to regex
		// arn:aws:iam::123456789012:role/my-role-* -> ^arn:aws:iam::123456789012:role/my-role-.*$
		pattern := "^" + regexp.QuoteMeta(role) + "$"
		pattern = strings.ReplaceAll(pattern, `\*`, ".*")
		re, err := regexp.Compile(pattern)
		if err != nil {
			return nil, fmt.Errorf("invalid role pattern %q: %w", role, err)
		}
		patterns = append(patterns, re)
	}

	return &AWSTokenExchanger{
		allowedRolePatterns: patterns,
		tokenTTL:            tokenTTL,
		signingKey:          signingKey,
		issuer:              issuer,
		audience:            issuer,
		debug:               strings.Contains(os.Getenv("DEBUG"), "auth") || strings.Contains(os.Getenv("DEBUG"), "all"),
		httpClient: &http.Client{
			Timeout: 10 * time.Second,
			CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
	}, nil
}

// TokenExchangeResponse is returned from the token exchange endpoint.
type TokenExchangeResponse struct {
	Token     string    `json:"token"`
	ExpiresAt time.Time `json:"expires_at"`
	TokenType string    `json:"token_type"`
}

// TokenHandler handles POST /auth/aws/token requests.
// The client provides a presigned STS GetCallerIdentity URL in the X-AWS-Auth header.
func (e *AWSTokenExchanger) TokenHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	presignedURL := r.Header.Get("X-AWS-Auth")
	if presignedURL == "" {
		http.Error(w, "missing X-AWS-Auth header", http.StatusBadRequest)
		return
	}

	// Execute the presigned GetCallerIdentity request
	identity, err := e.verifyPresignedURL(presignedURL)
	if err != nil {
		if e.debug {
			log.Printf("[aws-iam] presigned URL verification failed: %v", err)
		}
		http.Error(w, "invalid AWS credentials", http.StatusUnauthorized)
		return
	}

	// Check if the ARN matches allowed patterns
	if !e.isAllowed(identity.Arn) {
		if e.debug {
			log.Printf("[aws-iam] ARN %s not in allowlist", identity.Arn)
		}
		http.Error(w, "ARN not authorized", http.StatusForbidden)
		return
	}

	// Generate CXDB token
	token, expiresAt, err := e.generateToken(identity)
	if err != nil {
		if e.debug {
			log.Printf("[aws-iam] token generation failed: %v", err)
		}
		http.Error(w, "token generation failed", http.StatusInternalServerError)
		return
	}

	if e.debug {
		log.Printf("[aws-iam] issued token for %s (expires %s)", identity.Arn, expiresAt.Format(time.RFC3339))
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(TokenExchangeResponse{
		Token:     token,
		ExpiresAt: expiresAt,
		TokenType: "Bearer",
	})
}

// Verify validates a CXDB-issued AWS token and returns a Session.
func (e *AWSTokenExchanger) Verify(tokenString string) (*Session, error) {
	token, err := jwt.Parse([]byte(tokenString),
		jwt.WithKey(jwa.HS256, e.signingKey),
		jwt.WithValidate(true),
		jwt.WithIssuer(e.issuer),
		jwt.WithAudience(e.audience),
	)
	if err != nil {
		if e.debug {
			log.Printf("[aws-iam] token validation failed: %v", err)
		}
		return nil, fmt.Errorf("invalid token: %w", err)
	}

	// Check token type claim
	tokenType, _ := token.Get("cxdb:type")
	if tokenType != "aws_iam" {
		return nil, fmt.Errorf("wrong token type: %v", tokenType)
	}

	role, _ := token.Get("cxdb:role")
	roleStr, _ := role.(string)

	return &Session{
		ID:         fmt.Sprintf("aws:%s", token.Subject()),
		Email:      fmt.Sprintf("%s@aws.iam", roleStr),
		Name:       fmt.Sprintf("AWS IAM: %s", token.Subject()),
		Scopes:     []string{"cxdb:read", "cxdb:write"},
		CreatedAt:  token.IssuedAt(),
		ExpiresAt:  token.Expiration(),
		AuthMethod: "aws_sts",
		Issuer:     e.issuer,
		Subject:    token.Subject(),
	}, nil
}

// STSIdentity represents the response from GetCallerIdentity.
type STSIdentity struct {
	Account string `json:"Account"`
	Arn     string `json:"Arn"`
	UserId  string `json:"UserId"`
}

// verifyPresignedURL executes a presigned GetCallerIdentity request.
func (e *AWSTokenExchanger) verifyPresignedURL(presignedURL string) (*STSIdentity, error) {
	parsed, err := url.Parse(presignedURL)
	if err != nil {
		return nil, fmt.Errorf("parse presigned URL: %w", err)
	}
	if parsed.Scheme != "https" || parsed.User != nil || parsed.Port() != "" {
		return nil, fmt.Errorf("presigned URL must use HTTPS without userinfo or a custom port")
	}
	host := strings.ToLower(parsed.Hostname())
	if host != "sts.amazonaws.com" && !regexp.MustCompile(`^sts\.[a-z0-9-]+\.amazonaws\.com$`).MatchString(host) {
		return nil, fmt.Errorf("presigned URL host is not an AWS STS endpoint")
	}
	query := parsed.Query()
	action := query.Get("Action")
	if action == "" {
		action = query.Get("action")
	}
	if action != "GetCallerIdentity" {
		return nil, fmt.Errorf("presigned URL action must be GetCallerIdentity")
	}
	if query.Get("X-Amz-Signature") == "" && query.Get("x-amz-signature") == "" {
		return nil, fmt.Errorf("presigned URL is missing a signature")
	}

	req, err := http.NewRequest(http.MethodGet, presignedURL, nil)
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}

	resp, err := e.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("execute request: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("STS returned %d: %s", resp.StatusCode, string(body))
	}

	// STS returns XML by default, but presigned requests can specify JSON
	// We'll parse both formats
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("read response: %w", err)
	}

	// Try JSON first (if client requested it)
	var identity STSIdentity
	if err := json.Unmarshal(body, &identity); err == nil && identity.Arn != "" {
		return &identity, nil
	}

	// Parse XML response
	return parseSTSXMLResponse(body)
}

// parseSTSXMLResponse extracts identity from STS XML response.
func parseSTSXMLResponse(body []byte) (*STSIdentity, error) {
	// Simple extraction - STS response is well-formed
	s := string(body)

	extractTag := func(tag string) string {
		start := strings.Index(s, "<"+tag+">")
		if start == -1 {
			return ""
		}
		start += len(tag) + 2
		end := strings.Index(s[start:], "</"+tag+">")
		if end == -1 {
			return ""
		}
		return s[start : start+end]
	}

	arn := extractTag("Arn")
	if arn == "" {
		return nil, fmt.Errorf("no Arn in response")
	}

	return &STSIdentity{
		Arn:     arn,
		Account: extractTag("Account"),
		UserId:  extractTag("UserId"),
	}, nil
}

// isAllowed checks if an ARN matches any allowed pattern.
func (e *AWSTokenExchanger) isAllowed(arn string) bool {
	for _, pattern := range e.allowedRolePatterns {
		if pattern.MatchString(arn) {
			return true
		}
	}
	return false
}

// generateToken creates a signed JWT for the given identity.
func (e *AWSTokenExchanger) generateToken(identity *STSIdentity) (string, time.Time, error) {
	now := time.Now()
	expiresAt := now.Add(e.tokenTTL)

	// Extract role name from ARN
	roleName := extractRoleName(identity.Arn)

	token, err := jwt.NewBuilder().
		Issuer(e.issuer).
		Subject(identity.Arn).
		Audience([]string{e.audience}).
		IssuedAt(now).
		Expiration(expiresAt).
		Claim("cxdb:type", "aws_iam").
		Claim("cxdb:account", identity.Account).
		Claim("cxdb:role", roleName).
		Build()
	if err != nil {
		return "", time.Time{}, fmt.Errorf("build token: %w", err)
	}

	signed, err := jwt.Sign(token, jwt.WithKey(jwa.HS256, e.signingKey))
	if err != nil {
		return "", time.Time{}, fmt.Errorf("sign token: %w", err)
	}

	return string(signed), expiresAt, nil
}

// extractRoleName extracts the role name from an ARN.
// arn:aws:sts::123456789012:assumed-role/MyRole/session-name -> MyRole
// arn:aws:iam::123456789012:role/MyRole -> MyRole
func extractRoleName(arn string) string {
	parts := strings.Split(arn, "/")
	if len(parts) >= 2 {
		return filepath.Base(parts[1])
	}
	return arn
}
