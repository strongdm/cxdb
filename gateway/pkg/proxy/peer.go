// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"net"
	"net/http"
	"strings"
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

// rateLimitClientIP uses X-Forwarded-For only when the TCP peer is in an
// explicit trusted-proxy network. It walks the chain from right to left and
// returns the first untrusted address, which prevents client-supplied prefixes
// from changing the bucket key.
func rateLimitClientIP(req *http.Request, trusted []*net.IPNet) string {
	peer := observedPeerIP(req)
	peerIP := net.ParseIP(peer)
	if peerIP == nil || !ipInNetworks(peerIP, trusted) {
		return peer
	}
	forwarded := strings.Join(req.Header.Values("X-Forwarded-For"), ",")
	parts := strings.Split(forwarded, ",")
	for index := len(parts) - 1; index >= 0; index-- {
		candidate := net.ParseIP(strings.TrimSpace(parts[index]))
		if candidate == nil {
			return peer
		}
		if !ipInNetworks(candidate, trusted) {
			return candidate.String()
		}
	}
	return peer
}

func ipInNetworks(ip net.IP, networks []*net.IPNet) bool {
	for _, network := range networks {
		if network.Contains(ip) {
			return true
		}
	}
	return false
}
