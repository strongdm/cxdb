// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package auth

import (
	"net/http"
	"testing"
	"time"
)

func TestAWSPresignedURLRejectsUnsafeTargetsBeforeRequest(t *testing.T) {
	t.Parallel()
	exchanger, err := NewAWSTokenExchanger([]string{"*"}, time.Hour, []byte("test-signing-key"), "cxdb.example")
	if err != nil {
		t.Fatal(err)
	}
	exchanger.httpClient.Transport = roundTripFunc(func(*http.Request) (*http.Response, error) {
		t.Fatal("unsafe target reached the network")
		return nil, nil
	})

	for _, raw := range []string{
		"http://sts.amazonaws.com/?Action=GetCallerIdentity&X-Amz-Signature=x",
		"https://127.0.0.1/?Action=GetCallerIdentity&X-Amz-Signature=x",
		"https://sts.amazonaws.com:8443/?Action=GetCallerIdentity&X-Amz-Signature=x",
		"https://sts.amazonaws.com/?Action=DeleteIdentity&X-Amz-Signature=x",
		"https://sts.amazonaws.com/?Action=GetCallerIdentity",
	} {
		if _, err := exchanger.verifyPresignedURL(raw); err == nil {
			t.Errorf("verifyPresignedURL(%q) succeeded", raw)
		}
	}
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
