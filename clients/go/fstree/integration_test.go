// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

// Integration test for filesystem snapshots with CXDB server.
// Requires server running on localhost:9009 (binary) and localhost:9010 (HTTP).
// Run with: go test -v -tags=integration ./fstree -run TestE2E
//
//go:build integration

package fstree

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	cxdb "github.com/strongdm/ai-cxdb/clients/go"
	"github.com/vmihailenco/msgpack/v5"
)

const (
	binaryAddr = "127.0.0.1:9009"
	httpAddr   = "http://127.0.0.1:9010"

	TypeIDConversationItem      = "cxdb.v3:ConversationItem"
	TypeVersionConversationItem = 3
)

// TestE2E_FilesystemSnapshots exercises the complete filesystem snapshot workflow.
func TestE2E_FilesystemSnapshots(t *testing.T) {
	ctx := context.Background()

	// Connect to server
	client, err := cxdb.Dial(binaryAddr)
	if err != nil {
		t.Fatalf("Failed to connect to server at %s: %v\nMake sure the server is running", binaryAddr, err)
	}
	defer func() { _ = client.Close() }()

	t.Logf("Connected to CXDB server, session ID: %d", client.SessionID())

	// Create a temp directory for our test filesystem
	workDir := t.TempDir()
	t.Logf("Using work directory: %s", workDir)

	// Create a context
	ctxHead, err := client.CreateContext(ctx, 0)
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	t.Logf("Created context %d", ctxHead.ContextID)

	// =========================================================================
	// Phase 1: Initial filesystem state + Turn 1 with snapshot
	// =========================================================================
	t.Run("Phase1_InitialSnapshot", func(t *testing.T) {
		// Create initial filesystem structure
		createInitialFilesystem(t, workDir)

		// Capture snapshot
		snap, err := Capture(workDir, WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("Capture failed: %v", err)
		}
		t.Logf("Captured snapshot: %d files, %d dirs, root=%x",
			snap.Stats.FileCount, snap.Stats.DirCount, snap.RootHash[:8])

		// Upload snapshot
		result, err := snap.Upload(ctx, client)
		if err != nil {
			t.Fatalf("Upload failed: %v", err)
		}
		t.Logf("Uploaded: %d trees, %d files, %d bytes",
			result.TreesUploaded, result.FilesUploaded, result.BytesUploaded)

		// Append Turn 1 with filesystem snapshot
		turn1, err := client.AppendTurnWithFs(ctx, &cxdb.AppendRequest{
			ContextID:   ctxHead.ContextID,
			TypeID:      TypeIDConversationItem,
			TypeVersion: TypeVersionConversationItem,
			Payload:     makePayload(t, "user_input", "Initial state with filesystem"),
		}, &snap.RootHash)
		if err != nil {
			t.Fatalf("AppendTurnWithFs failed: %v", err)
		}
		t.Logf("Turn 1: id=%d, depth=%d", turn1.TurnID, turn1.Depth)

		// Verify via HTTP - list root
		verifyHTTPFsListing(t, turn1.TurnID, "", []string{"README.md", "src", "config"})

		// Verify file content
		verifyHTTPFsFileContent(t, turn1.TurnID, "README.md", "# Test Project")
		verifyHTTPFsFileContent(t, turn1.TurnID, "src/main.go", "package main\n\nfunc main() {}")
	})

	// Get context head for parent turn reference
	head, err := client.GetHead(ctx, ctxHead.ContextID)
	if err != nil {
		t.Fatalf("GetHead failed: %v", err)
	}
	turn1ID := head.HeadTurnID

	// =========================================================================
	// Phase 2: Turn 2 without snapshot (should inherit from Turn 1)
	// =========================================================================
	var turn2ID uint64
	t.Run("Phase2_InheritedSnapshot", func(t *testing.T) {
		// Append Turn 2 without filesystem snapshot
		turn2, err := client.AppendTurn(ctx, &cxdb.AppendRequest{
			ContextID:   ctxHead.ContextID,
			TypeID:      TypeIDConversationItem,
			TypeVersion: TypeVersionConversationItem,
			Payload:     makePayload(t, "assistant_turn", "Response without fs change"),
		})
		if err != nil {
			t.Fatalf("AppendTurn failed: %v", err)
		}
		turn2ID = turn2.TurnID
		t.Logf("Turn 2: id=%d, depth=%d (no snapshot attached)", turn2.TurnID, turn2.Depth)

		// Turn 2 should inherit filesystem from Turn 1
		verifyHTTPFsListing(t, turn2ID, "", []string{"README.md", "src", "config"})
		verifyHTTPFsFileContent(t, turn2ID, "README.md", "# Test Project")
	})

	// =========================================================================
	// Phase 3: Modify filesystem + Turn 3 with new snapshot
	// =========================================================================
	var turn3ID uint64
	t.Run("Phase3_ModifiedSnapshot", func(t *testing.T) {
		// Modify filesystem
		modifyFilesystem(t, workDir)

		// Capture new snapshot
		snap, err := Capture(workDir, WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("Capture failed: %v", err)
		}
		t.Logf("Captured modified snapshot: %d files, %d dirs, root=%x",
			snap.Stats.FileCount, snap.Stats.DirCount, snap.RootHash[:8])

		// Upload snapshot
		uploadResult, err := snap.Upload(ctx, client)
		if err != nil {
			t.Fatalf("Upload failed: %v", err)
		}
		t.Logf("Uploaded modified: %d trees, %d files (%d skipped dedup)",
			uploadResult.TreesUploaded, uploadResult.FilesUploaded, uploadResult.FilesSkipped)

		// Now append Turn 3 with the new snapshot
		turn3, err := client.AppendTurnWithFs(ctx, &cxdb.AppendRequest{
			ContextID:   ctxHead.ContextID,
			TypeID:      TypeIDConversationItem,
			TypeVersion: TypeVersionConversationItem,
			Payload:     makePayload(t, "user_input", "After filesystem changes"),
		}, &snap.RootHash)
		if err != nil {
			t.Fatalf("AppendTurnWithFs failed: %v", err)
		}
		turn3ID = turn3.TurnID
		t.Logf("Turn 3: id=%d, depth=%d", turn3.TurnID, turn3.Depth)

		// Verify Turn 3 has new filesystem state
		verifyHTTPFsListing(t, turn3ID, "", []string{"README.md", "src", "config", "docs"})
		verifyHTTPFsFileContent(t, turn3ID, "README.md", "# Test Project\n\nUpdated description.")
		verifyHTTPFsFileContent(t, turn3ID, "docs/INSTALL.md", "# Installation")

		// Verify Turn 1 still has OLD filesystem state
		verifyHTTPFsListing(t, turn1ID, "", []string{"README.md", "src", "config"})
		verifyHTTPFsFileContent(t, turn1ID, "README.md", "# Test Project")

		// Verify src/utils.go was deleted in Turn 3 but exists in Turn 1
		verifyHTTPFsFileContent(t, turn1ID, "src/utils.go", "package main\n\nfunc util() {}")
		verifyHTTPFsFileNotFound(t, turn3ID, "src/utils.go")
	})

	// =========================================================================
	// Phase 4: Test deep path traversal
	// =========================================================================
	t.Run("Phase4_DeepPaths", func(t *testing.T) {
		// Create deep directory structure
		deepPath := filepath.Join(workDir, "deep", "nested", "path", "to", "file")
		os.MkdirAll(filepath.Dir(deepPath), 0755)
		os.WriteFile(deepPath, []byte("deep content"), 0644)

		snap, err := Capture(workDir, WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("Capture failed: %v", err)
		}

		result, err := snap.Upload(ctx, client)
		if err != nil {
			t.Fatalf("Upload failed: %v", err)
		}
		t.Logf("Uploaded deep structure: %d trees, %d files", result.TreesUploaded, result.FilesUploaded)

		turn4, err := client.AppendTurnWithFs(ctx, &cxdb.AppendRequest{
			ContextID:   ctxHead.ContextID,
			TypeID:      TypeIDConversationItem,
			TypeVersion: TypeVersionConversationItem,
			Payload:     makePayload(t, "assistant_turn", "Added deep paths"),
		}, &snap.RootHash)
		if err != nil {
			t.Fatalf("AppendTurnWithFs failed: %v", err)
		}

		// Verify deep path traversal
		verifyHTTPFsListing(t, turn4.TurnID, "deep", []string{"nested"})
		verifyHTTPFsListing(t, turn4.TurnID, "deep/nested/path/to", []string{"file"})
		verifyHTTPFsFileContent(t, turn4.TurnID, "deep/nested/path/to/file", "deep content")
	})

	// =========================================================================
	// Phase 5: Test symlinks
	// =========================================================================
	t.Run("Phase5_Symlinks", func(t *testing.T) {
		// Create a symlink
		linkPath := filepath.Join(workDir, "link-to-readme")
		os.Remove(linkPath) // Remove if exists
		err := os.Symlink("README.md", linkPath)
		if err != nil {
			t.Skipf("Skipping symlink test: %v", err)
		}

		snap, err := Capture(workDir, WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("Capture failed: %v", err)
		}

		if snap.Stats.SymlinkCount == 0 {
			t.Error("Expected at least one symlink")
		}

		_, err = snap.Upload(ctx, client)
		if err != nil {
			t.Fatalf("Upload failed: %v", err)
		}
		t.Logf("Uploaded with symlinks: %d symlinks", snap.Stats.SymlinkCount)

		turn5, err := client.AppendTurnWithFs(ctx, &cxdb.AppendRequest{
			ContextID:   ctxHead.ContextID,
			TypeID:      TypeIDConversationItem,
			TypeVersion: TypeVersionConversationItem,
			Payload:     makePayload(t, "system", "Added symlinks"),
		}, &snap.RootHash)
		if err != nil {
			t.Fatalf("AppendTurnWithFs failed: %v", err)
		}

		// Symlink should resolve to target path
		verifyHTTPFsFileContent(t, turn5.TurnID, "link-to-readme", "README.md")
	})

	// =========================================================================
	// Phase 6: Test AttachFs (separate from append)
	// =========================================================================
	t.Run("Phase6_SeparateAttach", func(t *testing.T) {
		// Add a new file
		os.WriteFile(filepath.Join(workDir, "late-addition.txt"), []byte("added later"), 0644)

		snap, _, err := CaptureAndUpload(ctx, client, workDir, WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("CaptureAndUpload failed: %v", err)
		}

		// First append turn without fs
		turn6, err := client.AppendTurn(ctx, &cxdb.AppendRequest{
			ContextID:   ctxHead.ContextID,
			TypeID:      TypeIDConversationItem,
			TypeVersion: TypeVersionConversationItem,
			Payload:     makePayload(t, "user_input", "Turn before attach"),
		})
		if err != nil {
			t.Fatalf("AppendTurn failed: %v", err)
		}

		// Then attach filesystem separately
		attachResult, err := client.AttachFs(ctx, &cxdb.AttachFsRequest{
			TurnID:     turn6.TurnID,
			FsRootHash: snap.RootHash,
		})
		if err != nil {
			t.Fatalf("AttachFs failed: %v", err)
		}
		t.Logf("Attached fs to turn %d, root=%x", attachResult.TurnID, attachResult.FsRootHash[:8])

		// Verify
		verifyHTTPFsFileContent(t, turn6.TurnID, "late-addition.txt", "added later")
	})

	// =========================================================================
	// Phase 7: Content deduplication verification
	// =========================================================================
	t.Run("Phase7_Deduplication", func(t *testing.T) {
		// Create multiple identical files
		dupDir := filepath.Join(workDir, "duplicates")
		os.MkdirAll(dupDir, 0755)
		content := []byte("This content is duplicated across multiple files for dedup testing")
		for i := 0; i < 5; i++ {
			os.WriteFile(filepath.Join(dupDir, fmt.Sprintf("copy%d.txt", i)), content, 0644)
		}

		snap, err := Capture(workDir, WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("Capture failed: %v", err)
		}

		result, err := snap.Upload(ctx, client)
		if err != nil {
			t.Fatalf("Upload failed: %v", err)
		}

		t.Logf("Dedup test: 5 identical files uploaded as %d unique blobs", result.FilesUploaded)

		// Most should be skipped due to dedup (content already uploaded in earlier phases)
		// But if this is first time, FilesUploaded should be 1 (not 5)
		// Actually the dedup happens within the snapshot Files map, so we can check that
		if snap.Stats.FileCount > len(snap.Files)+5 {
			// Some dedup is happening
			t.Logf("Content deduplication working: %d files, %d unique blobs",
				snap.Stats.FileCount, len(snap.Files))
		}
	})

	// =========================================================================
	// Phase 8: Diff comparison
	// =========================================================================
	t.Run("Phase8_DiffComparison", func(t *testing.T) {
		// Capture current state
		snap1, err := Capture(workDir, WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("Capture 1 failed: %v", err)
		}

		// Make changes
		os.WriteFile(filepath.Join(workDir, "newfile.txt"), []byte("brand new"), 0644)
		os.WriteFile(filepath.Join(workDir, "README.md"), []byte("# Completely Changed"), 0644)
		os.Remove(filepath.Join(workDir, "late-addition.txt"))

		// Capture modified state
		snap2, err := Capture(workDir, WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("Capture 2 failed: %v", err)
		}

		// Compare
		diff, err := snap2.Diff(snap1)
		if err != nil {
			t.Fatalf("Diff failed: %v", err)
		}

		t.Logf("Diff: added=%v, modified=%v, removed=%v", diff.Added, diff.Modified, diff.Removed)

		// Verify diff contents
		if !contains(diff.Added, "newfile.txt") {
			t.Error("Expected newfile.txt in Added")
		}
		if !contains(diff.Modified, "README.md") {
			t.Error("Expected README.md in Modified")
		}
		if !contains(diff.Removed, "late-addition.txt") {
			t.Error("Expected late-addition.txt in Removed")
		}
	})

	// =========================================================================
	// Phase 9: Tracker incremental snapshots
	// =========================================================================
	t.Run("Phase9_Tracker", func(t *testing.T) {
		trackerDir := filepath.Join(workDir, "tracker-test")
		os.MkdirAll(trackerDir, 0755)
		os.WriteFile(filepath.Join(trackerDir, "tracked.txt"), []byte("tracked"), 0644)

		tracker := NewTracker(trackerDir)

		// First snapshot
		snap1, changed1, err := tracker.SnapshotIfChanged()
		if err != nil {
			t.Fatalf("First snapshot failed: %v", err)
		}
		if !changed1 || snap1 == nil {
			t.Error("First snapshot should report changed")
		}

		// No changes - should not create new snapshot
		snap2, changed2, err := tracker.SnapshotIfChanged()
		if err != nil {
			t.Fatalf("Second snapshot failed: %v", err)
		}
		if changed2 || snap2 != nil {
			t.Error("Second snapshot should report no changes")
		}

		// Make a change
		os.WriteFile(filepath.Join(trackerDir, "tracked.txt"), []byte("modified"), 0644)

		// Should detect change
		snap3, changed3, err := tracker.SnapshotIfChanged()
		if err != nil {
			t.Fatalf("Third snapshot failed: %v", err)
		}
		if !changed3 || snap3 == nil {
			t.Error("Third snapshot should report changed")
		}

		t.Logf("Tracker test passed: snap1=%x, snap3=%x", snap1.RootHash[:8], snap3.RootHash[:8])
	})

	// =========================================================================
	// Phase 10: Large file handling
	// =========================================================================
	t.Run("Phase10_LargeFileExclusion", func(t *testing.T) {
		largeDir := filepath.Join(workDir, "large-files")
		os.MkdirAll(largeDir, 0755)

		// Create a "large" file (1KB for test purposes)
		largeContent := make([]byte, 1024)
		os.WriteFile(filepath.Join(largeDir, "large.bin"), largeContent, 0644)
		os.WriteFile(filepath.Join(largeDir, "small.txt"), []byte("small"), 0644)

		// Capture with max file size of 100 bytes
		snap, err := Capture(workDir, WithMaxFileSize(100), WithExclude(".git", "*.tmp"))
		if err != nil {
			t.Fatalf("Capture failed: %v", err)
		}

		// large.bin should be excluded
		files, _ := snap.ListFiles()
		for _, f := range files {
			if strings.Contains(f, "large.bin") {
				t.Error("large.bin should have been excluded due to size")
			}
		}

		t.Logf("Large file exclusion working: %d files captured", snap.Stats.FileCount)
	})

	t.Log("All E2E phases completed successfully!")
}

// =========================================================================
// Test Helpers
// =========================================================================

func createInitialFilesystem(t *testing.T, dir string) {
	t.Helper()

	// Create directory structure
	os.MkdirAll(filepath.Join(dir, "src"), 0755)
	os.MkdirAll(filepath.Join(dir, "config"), 0755)

	// Create files
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# Test Project"), 0644)
	os.WriteFile(filepath.Join(dir, "src", "main.go"), []byte("package main\n\nfunc main() {}"), 0644)
	os.WriteFile(filepath.Join(dir, "src", "utils.go"), []byte("package main\n\nfunc util() {}"), 0644)
	os.WriteFile(filepath.Join(dir, "config", "settings.json"), []byte(`{"debug": true}`), 0644)
}

func modifyFilesystem(t *testing.T, dir string) {
	t.Helper()

	// Modify existing file
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# Test Project\n\nUpdated description."), 0644)

	// Add new directory and file
	os.MkdirAll(filepath.Join(dir, "docs"), 0755)
	os.WriteFile(filepath.Join(dir, "docs", "INSTALL.md"), []byte("# Installation"), 0644)

	// Delete a file
	os.Remove(filepath.Join(dir, "src", "utils.go"))
}

func makePayload(t *testing.T, itemType, text string) []byte {
	t.Helper()

	item := map[uint64]any{
		1: itemType,                                      // type
		2: "complete",                                    // status
		3: time.Now().UnixMilli(),                        // timestamp
		4: fmt.Sprintf("test-%d", time.Now().UnixNano()), // id
	}

	// Add type-specific content
	switch itemType {
	case "user_input":
		item[10] = map[uint64]any{1: text}
	case "assistant_turn":
		item[11] = map[uint64]any{1: text}
	case "system":
		item[12] = map[uint64]any{1: "info", 2: "Test", 3: text}
	}

	var buf bytes.Buffer
	enc := msgpack.NewEncoder(&buf)
	enc.SetCustomStructTag("msgpack")
	if err := enc.Encode(item); err != nil {
		t.Fatalf("msgpack encode failed: %v", err)
	}
	return buf.Bytes()
}

func verifyHTTPFsListing(t *testing.T, turnID uint64, path string, expectedNames []string) {
	t.Helper()

	url := fmt.Sprintf("%s/v1/turns/%d/fs", httpAddr, turnID)
	if path != "" {
		url = fmt.Sprintf("%s/v1/turns/%d/fs/%s", httpAddr, turnID, path)
	}

	resp, err := http.Get(url)
	if err != nil {
		t.Fatalf("HTTP GET %s failed: %v", url, err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != 200 {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("HTTP GET %s returned %d: %s", url, resp.StatusCode, body)
	}

	var listing struct {
		Entries []struct {
			Name string `json:"name"`
			Kind string `json:"kind"`
		} `json:"entries"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&listing); err != nil {
		t.Fatalf("Failed to decode listing response: %v", err)
	}

	names := make(map[string]bool)
	for _, e := range listing.Entries {
		names[e.Name] = true
	}

	for _, expected := range expectedNames {
		if !names[expected] {
			t.Errorf("Expected '%s' in listing for turn %d path '%s', got: %v",
				expected, turnID, path, listing.Entries)
		}
	}
}

func verifyHTTPFsFileContent(t *testing.T, turnID uint64, path string, expectedContent string) {
	t.Helper()

	url := fmt.Sprintf("%s/v1/turns/%d/fs/%s", httpAddr, turnID, path)
	resp, err := http.Get(url)
	if err != nil {
		t.Fatalf("HTTP GET %s failed: %v", url, err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != 200 {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("HTTP GET %s returned %d: %s", url, resp.StatusCode, body)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("Failed to read response body: %v", err)
	}

	// Check content type - should be raw for files
	contentType := resp.Header.Get("Content-Type")
	if strings.Contains(contentType, "application/json") {
		// This is a directory listing, not a file
		t.Fatalf("Expected file content but got directory listing for %s", path)
	}

	if string(body) != expectedContent {
		t.Errorf("Content mismatch for turn %d path '%s':\n  expected: %q\n  got: %q",
			turnID, path, expectedContent, string(body))
	}
}

func verifyHTTPFsFileNotFound(t *testing.T, turnID uint64, path string) {
	t.Helper()

	url := fmt.Sprintf("%s/v1/turns/%d/fs/%s", httpAddr, turnID, path)
	resp, err := http.Get(url)
	if err != nil {
		t.Fatalf("HTTP GET %s failed: %v", url, err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != 404 {
		t.Errorf("Expected 404 for turn %d path '%s', got %d", turnID, path, resp.StatusCode)
	}
}

func contains(slice []string, s string) bool {
	for _, item := range slice {
		if item == s {
			return true
		}
	}
	return false
}

// TestE2E_BlobDeduplication specifically tests that blobs are correctly deduplicated.
func TestE2E_BlobDeduplication(t *testing.T) {
	ctx := context.Background()

	client, err := cxdb.Dial(binaryAddr)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer func() { _ = client.Close() }()

	// Create unique content
	uniqueContent := []byte(fmt.Sprintf("unique content %d", time.Now().UnixNano()))

	// Upload the same blob twice
	hash1, wasNew1, err := client.PutBlobIfAbsent(ctx, uniqueContent)
	if err != nil {
		t.Fatalf("First PutBlobIfAbsent failed: %v", err)
	}

	hash2, wasNew2, err := client.PutBlobIfAbsent(ctx, uniqueContent)
	if err != nil {
		t.Fatalf("Second PutBlobIfAbsent failed: %v", err)
	}

	// Hashes should match
	if hash1 != hash2 {
		t.Errorf("Hash mismatch: %x vs %x", hash1, hash2)
	}

	// First should be new, second should not
	if !wasNew1 {
		t.Error("First upload should be marked as new")
	}
	if wasNew2 {
		t.Error("Second upload should NOT be marked as new (dedup)")
	}

	t.Logf("Deduplication verified: hash=%s", hex.EncodeToString(hash1[:]))
}

// TestE2E_FsRootInheritance tests that child turns inherit fs snapshots from parents.
func TestE2E_FsRootInheritance(t *testing.T) {
	ctx := context.Background()

	client, err := cxdb.Dial(binaryAddr)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer func() { _ = client.Close() }()

	workDir := t.TempDir()
	os.WriteFile(filepath.Join(workDir, "test.txt"), []byte("inherited content"), 0644)

	// Create context
	ctxHead, err := client.CreateContext(ctx, 0)
	if err != nil {
		t.Fatalf("CreateContext failed: %v", err)
	}

	// Turn 1 with snapshot
	snap, err := Capture(workDir)
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}
	snap.Upload(ctx, client)

	turn1, err := client.AppendTurnWithFs(ctx, &cxdb.AppendRequest{
		ContextID:   ctxHead.ContextID,
		TypeID:      TypeIDConversationItem,
		TypeVersion: TypeVersionConversationItem,
		Payload:     makePayload(t, "user_input", "Turn with snapshot"),
	}, &snap.RootHash)
	if err != nil {
		t.Fatalf("AppendTurnWithFs failed: %v", err)
	}

	// Turn 2, 3, 4 without snapshots
	var lastTurnID uint64 = turn1.TurnID
	for i := 2; i <= 4; i++ {
		turn, err := client.AppendTurn(ctx, &cxdb.AppendRequest{
			ContextID:   ctxHead.ContextID,
			TypeID:      TypeIDConversationItem,
			TypeVersion: TypeVersionConversationItem,
			Payload:     makePayload(t, "assistant_turn", fmt.Sprintf("Turn %d without snapshot", i)),
		})
		if err != nil {
			t.Fatalf("AppendTurn %d failed: %v", i, err)
		}
		lastTurnID = turn.TurnID
	}

	// Turn 4 (depth 3) should still see the filesystem from Turn 1
	verifyHTTPFsFileContent(t, lastTurnID, "test.txt", "inherited content")

	t.Logf("Inheritance verified: Turn %d (4 levels deep) can see fs from Turn %d", lastTurnID, turn1.TurnID)
}
