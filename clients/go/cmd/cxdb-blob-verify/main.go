// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

// cxdb-blob-verify is a one-off verification tool that exercises PutBlob and
// GetBlob against a live CXDB server. It verifies end-to-end correctness of
// the blob CAS round-trip, including content integrity via BLAKE3 hashing.
//
// Usage:
//
//	cxdb-blob-verify -addr 127.0.0.1:9009
//	CXDB_PORT=9009 cxdb-blob-verify
package main

import (
	"bytes"
	"context"
	"encoding/hex"
	"errors"
	"flag"
	"fmt"
	"os"
	"time"

	cxdb "github.com/strongdm/ai-cxdb/clients/go"
	"github.com/zeebo/blake3"
)

var addr string

func dial() *cxdb.Client {
	client, err := cxdb.Dial(addr, cxdb.WithClientTag("blob-verify"))
	if err != nil {
		fmt.Fprintf(os.Stderr, "  dial failed: %v\n", err)
		os.Exit(2)
	}
	return client
}

func main() {
	flag.StringVar(&addr, "addr", "", "CXDB server address (host:port). Falls back to 127.0.0.1:$CXDB_PORT")
	flag.Parse()

	if addr == "" {
		port := os.Getenv("CXDB_PORT")
		if port == "" {
			fmt.Fprintln(os.Stderr, "error: no -addr flag and CXDB_PORT not set")
			os.Exit(1)
		}
		addr = "127.0.0.1:" + port
	}

	fmt.Printf("server: %s\n\n", addr)

	passed := 0
	failed := 0

	run := func(name string, fn func() error) {
		fmt.Printf("=== %s ===\n", name)
		if err := fn(); err != nil {
			fmt.Fprintf(os.Stderr, "  FAIL: %v\n", err)
			failed++
		} else {
			fmt.Println("  PASS")
			passed++
		}
		fmt.Println()
	}

	// --- Test 1: PutBlob + GetBlob round-trip ---
	run("PutBlob + GetBlob round-trip", func() error {
		client := dial()
		defer func() { _ = client.Close() }()

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		testData := []byte(fmt.Sprintf("cxdb-blob-verify test payload %d", time.Now().UnixNano()))
		expectedHash := blake3.Sum256(testData)

		result, err := client.PutBlob(ctx, &cxdb.PutBlobRequest{Data: testData})
		if err != nil {
			return fmt.Errorf("PutBlob: %w", err)
		}
		fmt.Printf("  PutBlob: hash=%s was_new=%v\n", hex.EncodeToString(result.Hash[:]), result.WasNew)
		if result.Hash != expectedHash {
			return fmt.Errorf("hash mismatch: got %x, want %x", result.Hash, expectedHash)
		}

		retrieved, err := client.GetBlob(ctx, expectedHash)
		if err != nil {
			return fmt.Errorf("GetBlob: %w", err)
		}
		if !bytes.Equal(retrieved, testData) {
			return fmt.Errorf("content mismatch: got %d bytes, want %d bytes", len(retrieved), len(testData))
		}
		fmt.Printf("  GetBlob: %d bytes, content matches\n", len(retrieved))
		return nil
	})

	// --- Test 2: GetBlob nonexistent hash returns ErrBlobNotFound ---
	run("GetBlob nonexistent hash (expect ErrBlobNotFound)", func() error {
		client := dial()
		defer func() { _ = client.Close() }()

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		var bogusHash [32]byte
		for i := range bogusHash {
			bogusHash[i] = 0xDE
		}
		_, err := client.GetBlob(ctx, bogusHash)
		if err == nil {
			return fmt.Errorf("expected error for nonexistent blob, got nil")
		}
		if !errors.Is(err, cxdb.ErrBlobNotFound) {
			return fmt.Errorf("expected ErrBlobNotFound, got: %v (type %T)", err, err)
		}
		fmt.Printf("  got expected ErrBlobNotFound: %v\n", err)
		return nil
	})

	// --- Test 3: PutBlobIfAbsent deduplication ---
	run("PutBlobIfAbsent deduplication", func() error {
		client := dial()
		defer func() { _ = client.Close() }()

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		dedupData := []byte("dedup-test-payload-" + fmt.Sprint(time.Now().UnixNano()))

		hash1, wasNew1, err := client.PutBlobIfAbsent(ctx, dedupData)
		if err != nil {
			return fmt.Errorf("first PutBlobIfAbsent: %w", err)
		}
		fmt.Printf("  first put: hash=%s was_new=%v\n", hex.EncodeToString(hash1[:]), wasNew1)
		if !wasNew1 {
			return fmt.Errorf("first put should be new")
		}

		hash2, wasNew2, err := client.PutBlobIfAbsent(ctx, dedupData)
		if err != nil {
			return fmt.Errorf("second PutBlobIfAbsent: %w", err)
		}
		fmt.Printf("  second put: hash=%s was_new=%v\n", hex.EncodeToString(hash2[:]), wasNew2)
		if wasNew2 {
			return fmt.Errorf("second put should NOT be new (dedup)")
		}
		if hash1 != hash2 {
			return fmt.Errorf("hashes differ: %x vs %x", hash1, hash2)
		}

		// Verify we can get it back
		retrieved, err := client.GetBlob(ctx, hash1)
		if err != nil {
			return fmt.Errorf("GetBlob after dedup: %w", err)
		}
		if !bytes.Equal(retrieved, dedupData) {
			return fmt.Errorf("dedup content mismatch")
		}
		fmt.Printf("  GetBlob after dedup: %d bytes, content matches\n", len(retrieved))
		return nil
	})

	// --- Test 4: Large blob (1 MiB) ---
	run("Large blob (1 MiB) round-trip", func() error {
		client := dial()
		defer func() { _ = client.Close() }()

		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		largeData := make([]byte, 1024*1024)
		for i := range largeData {
			largeData[i] = byte(i % 251)
		}

		largeHash, _, err := client.PutBlobIfAbsent(ctx, largeData)
		if err != nil {
			return fmt.Errorf("PutBlob large: %w", err)
		}
		fmt.Printf("  PutBlob: hash=%s\n", hex.EncodeToString(largeHash[:]))

		retrieved, err := client.GetBlob(ctx, largeHash)
		if err != nil {
			return fmt.Errorf("GetBlob large: %w", err)
		}
		if !bytes.Equal(retrieved, largeData) {
			return fmt.Errorf("large blob content mismatch: got %d bytes, want %d", len(retrieved), len(largeData))
		}
		fmt.Printf("  GetBlob: %d bytes, content verified\n", len(retrieved))
		return nil
	})

	// --- Test 5: Connection survives a not-found ---
	run("Connection survives not-found", func() error {
		client := dial()
		defer func() { _ = client.Close() }()

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		// Step 1: PutBlob proves the connection works
		data1 := []byte(fmt.Sprintf("survive-test-1-%d", time.Now().UnixNano()))
		_, err := client.PutBlob(ctx, &cxdb.PutBlobRequest{Data: data1})
		if err != nil {
			return fmt.Errorf("step 1 PutBlob: %w", err)
		}
		fmt.Println("  step 1: PutBlob succeeded")

		// Step 2: GetBlob for nonexistent hash (used to kill the connection)
		var bogusHash [32]byte
		for i := range bogusHash {
			bogusHash[i] = 0xAB
		}
		_, err = client.GetBlob(ctx, bogusHash)
		if !errors.Is(err, cxdb.ErrBlobNotFound) {
			return fmt.Errorf("step 2: expected ErrBlobNotFound, got: %v", err)
		}
		fmt.Println("  step 2: GetBlob not-found returned proper error")

		// Step 3: PutBlob + GetBlob for real data (proves connection is STILL ALIVE)
		data2 := []byte(fmt.Sprintf("survive-test-2-%d", time.Now().UnixNano()))
		hash2 := blake3.Sum256(data2)
		_, err = client.PutBlob(ctx, &cxdb.PutBlobRequest{Data: data2})
		if err != nil {
			return fmt.Errorf("step 3 PutBlob: %w (connection died after not-found)", err)
		}
		retrieved, err := client.GetBlob(ctx, hash2)
		if err != nil {
			return fmt.Errorf("step 3 GetBlob: %w (connection died after not-found)", err)
		}
		if !bytes.Equal(retrieved, data2) {
			return fmt.Errorf("step 3: content mismatch")
		}
		fmt.Println("  step 3: PutBlob + GetBlob succeeded — connection survived!")
		return nil
	})

	// --- Test 6: ReconnectingClient handles not-found without reconnecting ---
	run("ReconnectingClient not-found without reconnect", func() error {
		rc, err := cxdb.DialReconnecting(addr, nil, cxdb.WithClientTag("blob-verify-reconnect"))
		if err != nil {
			return fmt.Errorf("DialReconnecting: %w", err)
		}
		defer func() { _ = rc.Close() }()

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		sessionBefore := rc.SessionID()
		fmt.Printf("  session before: %d\n", sessionBefore)

		// GetBlob for nonexistent hash should return ErrBlobNotFound
		var bogusHash [32]byte
		for i := range bogusHash {
			bogusHash[i] = 0xCD
		}
		_, err = rc.GetBlob(ctx, bogusHash)
		if !errors.Is(err, cxdb.ErrBlobNotFound) {
			return fmt.Errorf("expected ErrBlobNotFound, got: %v", err)
		}
		fmt.Printf("  GetBlob not-found returned proper error\n")

		sessionAfter := rc.SessionID()
		fmt.Printf("  session after: %d\n", sessionAfter)
		if sessionBefore != sessionAfter {
			return fmt.Errorf("session ID changed from %d to %d — reconnection happened (should not for 404)", sessionBefore, sessionAfter)
		}
		fmt.Println("  session ID unchanged — no reconnection")

		// Prove client still works with a real PutBlob + GetBlob
		realData := []byte(fmt.Sprintf("reconnect-test-%d", time.Now().UnixNano()))
		realHash := blake3.Sum256(realData)
		_, err = rc.PutBlob(ctx, &cxdb.PutBlobRequest{Data: realData})
		if err != nil {
			return fmt.Errorf("PutBlob after not-found: %w", err)
		}
		retrieved, err := rc.GetBlob(ctx, realHash)
		if err != nil {
			return fmt.Errorf("GetBlob after not-found: %w", err)
		}
		if !bytes.Equal(retrieved, realData) {
			return fmt.Errorf("content mismatch after not-found")
		}
		fmt.Println("  PutBlob + GetBlob after not-found succeeded")
		return nil
	})

	// --- Summary ---
	fmt.Printf("=== Results: %d passed, %d failed ===\n", passed, failed)
	if failed > 0 {
		os.Exit(1)
	}
}
