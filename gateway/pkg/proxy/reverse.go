// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"log/slog"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"
	"time"
)

// ReverseProxy wraps httputil.ReverseProxy with additional configuration.
type ReverseProxy struct {
	proxy  *httputil.ReverseProxy
	target *url.URL
	logger *slog.Logger
}

// NewReverseProxy creates a reverse proxy to the specified backend URL.
func NewReverseProxy(backendURL string, logger *slog.Logger) (*ReverseProxy, error) {
	target, err := url.Parse(backendURL)
	if err != nil {
		return nil, err
	}

	proxy := &httputil.ReverseProxy{}

	// Use Rewrite (Go 1.20+). Rewrite is mutually exclusive with Director
	// and disables the stdlib's default `X-Forwarded-For` auto-append —
	// essential for the Sprint 019 / ADR-006 trust contract: the gateway
	// MUST be the sole writer of `X-Forwarded-For` on outbound requests.
	proxy.Rewrite = func(r *httputil.ProxyRequest) {
		// Point the outbound request at the target backend.
		r.SetURL(target)
		r.Out.Host = target.Host

		// XFF trust contract (Sprint 019 / ADR-006): the gateway is the
		// trust boundary. DROP any caller-supplied `X-Forwarded-For` and
		// `Forwarded` headers first — they are attacker-controllable.
		// Then set `X-Forwarded-For` to our own TCP-peer observation. The
		// `observedPeerIP` helper is the single source of real-client-IP
		// truth across the gateway (logging, rate-limit, this director).
		r.Out.Header.Del("X-Forwarded-For")
		r.Out.Header.Del("Forwarded")
		// Identity headers are gateway assertions. Never forward caller values.
		for header := range r.Out.Header {
			if strings.HasPrefix(strings.ToLower(header), "x-cxdb-") {
				r.Out.Header.Del(header)
			}
		}
		// Authentication is complete at the gateway. Do not forward browser
		// cookies or bearer credentials to the Rust backend.
		r.Out.Header.Del("Authorization")
		r.Out.Header.Del("Cookie")
		r.Out.Header.Set("X-Forwarded-For", observedPeerIP(r.In))

		// X-Forwarded-Proto is a gateway assertion. Never preserve a caller value.
		r.Out.Header.Del("X-Forwarded-Proto")
		if r.In.TLS != nil {
			r.Out.Header.Set("X-Forwarded-Proto", "https")
		} else {
			r.Out.Header.Set("X-Forwarded-Proto", "http")
		}

		// The Rust backend does not need the public host. Drop this header so a
		// caller-selected Host value cannot cross the gateway trust boundary.
		r.Out.Header.Del("X-Forwarded-Host")
	}

	// Custom error handler
	proxy.ErrorHandler = func(w http.ResponseWriter, r *http.Request, err error) {
		logger.Error("proxy error", "path", r.URL.Path, "method", r.Method, "err", err)
		http.Error(w, "Bad Gateway", http.StatusBadGateway)
	}

	// Custom transport with reasonable timeouts
	proxy.Transport = &http.Transport{
		DialContext: (&net.Dialer{
			Timeout:   30 * time.Second,
			KeepAlive: 30 * time.Second,
		}).DialContext,
		MaxIdleConns:          100,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ExpectContinueTimeout: 1 * time.Second,
	}

	return &ReverseProxy{
		proxy:  proxy,
		target: target,
		logger: logger,
	}, nil
}

// ServeHTTP implements http.Handler.
func (rp *ReverseProxy) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	rp.proxy.ServeHTTP(w, r)
}

// Target returns the backend URL.
func (rp *ReverseProxy) Target() string {
	return rp.target.String()
}
