// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"context"
	"fmt"
	"io/fs"
	"log/slog"
	"net"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/strongdm/cxdb/gateway/internal/config"
	"github.com/strongdm/cxdb/gateway/pkg/auth"
	"golang.org/x/time/rate"
)

// Server wires together config, auth, and the reverse proxy.
type Server struct {
	cfg      config.Config
	mux      *http.ServeMux
	sessions *auth.SessionStore
	google   *auth.GoogleAuth
	proxy    *ReverseProxy
	sse      *SSEBroker
	logger   *slog.Logger
	staticFS fs.FS

	cspHeader   string
	hstsEnabled bool
	limiters    *ipRateLimiter

	// Service-to-service auth verifiers (optional)
	tokenVerifiers []auth.BearerTokenVerifier
	awsExchanger   *auth.AWSTokenExchanger
}

// New constructs the HTTP server and registers all routes.
func New(cfg config.Config, sessions *auth.SessionStore, google *auth.GoogleAuth, proxy *ReverseProxy, staticFS fs.FS, logger *slog.Logger) (*Server, error) {
	mux := http.NewServeMux()

	// Create SSE broker for live events
	sseBroker := NewSSEBroker(proxy.Target(), logger)

	// Build CSP header with dynamic renderer origins
	scriptSrc := "'self' 'unsafe-inline'"
	for _, origin := range cfg.AllowedRendererOrigins {
		scriptSrc += " " + origin
	}

	s := &Server{
		cfg:      cfg,
		mux:      mux,
		sessions: sessions,
		google:   google,
		proxy:    proxy,
		sse:      sseBroker,
		logger:   logger,
		staticFS: staticFS,
		cspHeader: strings.Join([]string{
			"default-src 'self'",
			"img-src 'self' data: https://lh3.googleusercontent.com",
			"script-src " + scriptSrc,
			"style-src 'self' 'unsafe-inline'",
			"connect-src 'self'",
			"frame-ancestors 'none'",
			"form-action 'self' https://accounts.google.com https://*.google.com",
			"base-uri 'self'",
		}, "; "),
		hstsEnabled: strings.HasPrefix(strings.ToLower(cfg.PublicBaseURL), "https://"),
		limiters:    newIPRateLimiter(rate.Limit(5), 10),
	}

	// Initialize K8s OIDC verifier if enabled
	if cfg.K8sOIDCEnabled {
		k8sVerifier, err := auth.NewK8sOIDCVerifier(
			cfg.K8sOIDCIssuerURL,
			cfg.K8sOIDCAudience,
			cfg.K8sOIDCAllowedNamespaces,
		)
		if err != nil {
			return nil, fmt.Errorf("init k8s oidc verifier: %w", err)
		}
		s.tokenVerifiers = append(s.tokenVerifiers, k8sVerifier)
		logger.Info("k8s_oidc_enabled", "issuer", cfg.K8sOIDCIssuerURL, "audience", cfg.K8sOIDCAudience)
	}

	// Initialize AWS IAM token exchanger if enabled
	if cfg.AWSIAMEnabled {
		// Extract issuer from PublicBaseURL (e.g., "https://your-domain.com" -> "your-domain.com")
		issuer := strings.TrimPrefix(cfg.PublicBaseURL, "https://")
		issuer = strings.TrimPrefix(issuer, "http://")
		issuer = strings.TrimSuffix(issuer, "/")

		awsExchanger, err := auth.NewAWSTokenExchanger(
			cfg.AWSIAMAllowedRoles,
			cfg.AWSIAMTokenTTL,
			[]byte(cfg.SessionSecret),
			issuer,
		)
		if err != nil {
			return nil, fmt.Errorf("init aws iam exchanger: %w", err)
		}
		s.awsExchanger = awsExchanger
		s.tokenVerifiers = append(s.tokenVerifiers, awsExchanger)
		logger.Info("aws_iam_enabled", "allowed_roles", len(cfg.AWSIAMAllowedRoles), "token_ttl", cfg.AWSIAMTokenTTL)
	}

	// Health check endpoints (public)
	mux.HandleFunc("/healthz", s.healthz)
	mux.HandleFunc("/readyz", s.readyz)

	// OAuth endpoints (public)
	mux.HandleFunc("/auth/google/login", google.LoginHandler)
	mux.HandleFunc("/auth/google/callback", google.CallbackHandler)
	mux.HandleFunc("/auth/google/logout", google.LogoutHandler)

	// AWS IAM token exchange endpoint (public - uses AWS creds for auth)
	if s.awsExchanger != nil {
		mux.HandleFunc("/auth/aws/token", s.awsExchanger.TokenHandler)
	}

	// API info endpoint
	mux.HandleFunc("/api/v1/me", s.me)

	// SSE endpoint for live events (must be before /v1/ catch-all)
	mux.Handle("/v1/events", sseBroker)

	// Reverse proxy for all /v1/* endpoints
	mux.Handle("/v1/", proxy)

	// Serve embedded React frontend for all other routes
	mux.Handle("/", s.staticHandler())

	return s, nil
}

// ListenAndServe starts the HTTP server and blocks until it exits.
func (s *Server) ListenAndServe(ctx context.Context) error {
	// Start SSE broker polling
	s.sse.Start(ctx)

	addr := fmt.Sprintf(":%s", s.cfg.Port)
	handler := auth.RequireAuthForReadsWithOptions(auth.AuthMiddlewareOptions{
		Store:          s.sessions,
		DevBypass:      s.cfg.DevMode,
		TokenVerifiers: s.tokenVerifiers,
	}, s.mux)
	handler = s.rateLimitMiddleware(handler)
	handler = s.securityHeaders(handler)
	handler = s.loggingMiddleware(handler)

	srv := &http.Server{
		Addr:         addr,
		Handler:      handler,
		ReadTimeout:  10 * time.Second,
		WriteTimeout: 120 * time.Second, // Longer for SSE connections
		IdleTimeout:  120 * time.Second,
	}

	go func() {
		<-ctx.Done()
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		if err := srv.Shutdown(shutdownCtx); err != nil {
			s.logger.Error("server shutdown error", "err", err)
		}
	}()

	s.logger.Info("http_server_listening", "addr", addr, "backend", s.proxy.Target())
	if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		return err
	}
	return nil
}

func (s *Server) healthz(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte("ok"))
}

func (s *Server) readyz(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithTimeout(r.Context(), 500*time.Millisecond)
	defer cancel()

	if err := s.sessions.Ping(ctx); err != nil {
		s.logger.Error("readyz database ping failed", "err", err)
		http.Error(w, "not ready", http.StatusServiceUnavailable)
		return
	}
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte("ready"))
}

func (s *Server) me(w http.ResponseWriter, r *http.Request) {
	user := auth.UserFromContext(r.Context())
	if user == nil {
		http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"email":%q,"name":%q,"picture":%q}`, user.Email, user.Name, user.Picture)
}

// staticHandler serves the embedded React frontend with smart routing for Next.js static export.
func (s *Server) staticHandler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path := strings.TrimPrefix(r.URL.Path, "/")

		// Handle root - serve index.html
		if path == "" {
			http.ServeFileFS(w, r, s.staticFS, "index.html")
			return
		}

		// Normalize: strip trailing slash
		cleanPath := strings.TrimSuffix(path, "/")

		// Next.js static export creates {route}.html files (e.g., /login -> login.html)
		if !strings.Contains(cleanPath, ".") {
			htmlPath := cleanPath + ".html"
			if _, err := fs.Stat(s.staticFS, htmlPath); err == nil {
				http.ServeFileFS(w, r, s.staticFS, htmlPath)
				return
			}
		}

		// Exact path for static assets (.js, .css, images, etc.)
		if _, err := fs.Stat(s.staticFS, path); err == nil {
			http.ServeFileFS(w, r, s.staticFS, path)
			return
		}

		// Directory-style routes (e.g., /research/foo/ -> research/foo/index.html)
		indexPath := cleanPath + "/index.html"
		if _, err := fs.Stat(s.staticFS, indexPath); err == nil {
			http.ServeFileFS(w, r, s.staticFS, indexPath)
			return
		}

		// Fallback: serve index.html for client-side routing (SPA behavior)
		// This allows React Router to handle unknown routes
		http.ServeFileFS(w, r, s.staticFS, "index.html")
	})
}

// securityHeaders adds production-grade headers including CSP and HSTS.
// Note: SSE endpoint is excluded since streaming responses have different requirements.
func (s *Server) securityHeaders(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Skip security headers for SSE endpoint - they can interfere with streaming
		if r.URL.Path == "/v1/events" {
			next.ServeHTTP(w, r)
			return
		}

		h := w.Header()
		h.Set("Content-Security-Policy", s.cspHeader)
		h.Set("Referrer-Policy", "no-referrer")
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("X-Frame-Options", "DENY")
		h.Set("Permissions-Policy", "geolocation=(), microphone=(), camera=()")
		if s.hstsEnabled {
			h.Set("Strict-Transport-Security", "max-age=31536000; includeSubDomains; preload")
		}
		next.ServeHTTP(w, r)
	})
}

// rateLimitMiddleware throttles repeated auth hits to protect OAuth endpoints.
func (s *Server) rateLimitMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !shouldRateLimit(r.URL.Path) {
			next.ServeHTTP(w, r)
			return
		}
		ip := clientIP(r)
		limiter := s.limiters.get(ip)
		if !limiter.Allow() {
			s.logger.Warn("rate_limit_exceeded", "ip", ip, "path", r.URL.Path)
			http.Error(w, "too many requests", http.StatusTooManyRequests)
			return
		}
		next.ServeHTTP(w, r)
	})
}

// loggingMiddleware emits structured JSON logs for each request.
func (s *Server) loggingMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Skip wrapping for SSE endpoint - the wrapper can interfere with HTTP/2 streaming
		if r.URL.Path == "/v1/events" {
			s.logger.Info("http_sse_start", "method", r.Method, "path", r.URL.Path, "ip", clientIP(r))
			next.ServeHTTP(w, r)
			s.logger.Info("http_sse_end", "method", r.Method, "path", r.URL.Path)
			return
		}

		start := time.Now()
		sw := &statusWriter{ResponseWriter: w, status: http.StatusOK}

		next.ServeHTTP(sw, r)

		user := ""
		if u := auth.UserFromContext(r.Context()); u != nil {
			user = u.Email
		}
		s.logger.Info("http_request",
			"method", r.Method,
			"path", r.URL.Path,
			"status", sw.status,
			"duration_ms", time.Since(start).Milliseconds(),
			"size_bytes", sw.bytes,
			"ip", clientIP(r),
			"user", user,
		)
	})
}

type statusWriter struct {
	http.ResponseWriter
	status int
	bytes  int64
}

func (w *statusWriter) WriteHeader(statusCode int) {
	w.status = statusCode
	w.ResponseWriter.WriteHeader(statusCode)
}

func (w *statusWriter) Write(b []byte) (int, error) {
	n, err := w.ResponseWriter.Write(b)
	w.bytes += int64(n)
	return n, err
}

// Flush implements http.Flusher for SSE support
func (w *statusWriter) Flush() {
	if f, ok := w.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

func clientIP(r *http.Request) string {
	xff := r.Header.Get("X-Forwarded-For")
	if xff != "" {
		parts := strings.Split(xff, ",")
		return strings.TrimSpace(parts[0])
	}
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

type ipRateLimiter struct {
	mu       sync.Mutex
	visitors map[string]*rate.Limiter
	r        rate.Limit
	burst    int
}

func newIPRateLimiter(r rate.Limit, burst int) *ipRateLimiter {
	return &ipRateLimiter{
		visitors: make(map[string]*rate.Limiter),
		r:        r,
		burst:    burst,
	}
}

func (l *ipRateLimiter) get(ip string) *rate.Limiter {
	l.mu.Lock()
	defer l.mu.Unlock()
	limiter, ok := l.visitors[ip]
	if !ok {
		limiter = rate.NewLimiter(l.r, l.burst)
		l.visitors[ip] = limiter
	}
	return limiter
}

func shouldRateLimit(path string) bool {
	path = strings.ToLower(path)
	if path == "/login" || strings.HasPrefix(path, "/auth/") {
		return true
	}
	return false
}
