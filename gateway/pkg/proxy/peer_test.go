// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"net"
	"net/http/httptest"
	"testing"
)

func TestRateLimitClientIPTrustBoundary(t *testing.T) {
	_, trusted, err := net.ParseCIDR("10.0.0.0/8")
	if err != nil {
		t.Fatal(err)
	}
	tests := []struct {
		name       string
		remoteAddr string
		forwarded  string
		want       string
	}{
		{name: "direct client ignores header", remoteAddr: "198.51.100.7:1234", forwarded: "203.0.113.9", want: "198.51.100.7"},
		{name: "trusted proxy uses client", remoteAddr: "10.0.0.4:1234", forwarded: "203.0.113.9", want: "203.0.113.9"},
		{name: "trusted chain skips trusted hops", remoteAddr: "10.0.0.4:1234", forwarded: "192.0.2.8, 203.0.113.9, 10.1.2.3", want: "203.0.113.9"},
		{name: "malformed chain fails closed", remoteAddr: "10.0.0.4:1234", forwarded: "203.0.113.9, invalid", want: "10.0.0.4"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			request := httptest.NewRequest("GET", "https://cxdb.example/login", nil)
			request.RemoteAddr = test.remoteAddr
			request.Header.Set("X-Forwarded-For", test.forwarded)
			if got := rateLimitClientIP(request, []*net.IPNet{trusted}); got != test.want {
				t.Fatalf("rateLimitClientIP() = %q, want %q", got, test.want)
			}
		})
	}
}
