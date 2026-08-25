// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"net"
	"net/http"
)

// observedPeerIP returns the TCP peer's address parsed from req.RemoteAddr.
//
// This helper is the sole source of "real client address" at the gateway
// trust boundary. It NEVER reads any HTTP header. Every gateway site that
// needs a real peer (reverse-proxy XFF write, request logging, rate-limit
// bucket key, debug-auth IP allowlist) MUST call this helper and MUST NOT
// inspect `X-Forwarded-For` or `Forwarded` directly — those headers are
// attacker-controllable (see ADR-006).
//
// If `RemoteAddr` lacks a port (unusual — net/http populates it with
// "host:port"), the raw value is returned unchanged rather than mangled.
func observedPeerIP(req *http.Request) string {
	host, _, err := net.SplitHostPort(req.RemoteAddr)
	if err != nil {
		return req.RemoteAddr
	}
	return host
}
