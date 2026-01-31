// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package fstree

import (
	"os"
	"path/filepath"
	"testing"
)

func TestCapture_BasicTree(t *testing.T) {
	// Create a temp directory with a simple structure
	tmpDir := t.TempDir()

	// Create files and directories
	_ = os.MkdirAll(filepath.Join(tmpDir, "src"), 0755)
	_ = os.WriteFile(filepath.Join(tmpDir, "README.md"), []byte("# Test"), 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "src", "main.go"), []byte("package main"), 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "src", "lib.go"), []byte("package main\n\nfunc foo() {}"), 0644)

	// Capture
	snap, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}

	// Verify stats
	if snap.Stats.FileCount != 3 {
		t.Errorf("expected 3 files, got %d", snap.Stats.FileCount)
	}
	if snap.Stats.DirCount != 2 { // root + src
		t.Errorf("expected 2 directories, got %d", snap.Stats.DirCount)
	}

	// Verify we can list files
	files, err := snap.ListFiles()
	if err != nil {
		t.Fatalf("ListFiles failed: %v", err)
	}
	if len(files) != 3 {
		t.Errorf("expected 3 files, got %d: %v", len(files), files)
	}
}

func TestCapture_DeterministicHash(t *testing.T) {
	tmpDir := t.TempDir()

	_ = os.WriteFile(filepath.Join(tmpDir, "a.txt"), []byte("hello"), 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "b.txt"), []byte("world"), 0644)

	// Capture twice
	snap1, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture 1 failed: %v", err)
	}

	snap2, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture 2 failed: %v", err)
	}

	// Root hashes should be identical
	if snap1.RootHash != snap2.RootHash {
		t.Errorf("root hashes differ:\n  snap1: %x\n  snap2: %x", snap1.RootHash, snap2.RootHash)
	}
}

func TestCapture_ContentAddressing(t *testing.T) {
	tmpDir := t.TempDir()

	// Two files with same content
	content := []byte("identical content")
	_ = os.WriteFile(filepath.Join(tmpDir, "file1.txt"), content, 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "file2.txt"), content, 0644)

	snap, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}

	// Should have 2 files tracked but only 1 unique blob
	if snap.Stats.FileCount != 2 {
		t.Errorf("expected 2 files, got %d", snap.Stats.FileCount)
	}
	if len(snap.Files) != 1 {
		t.Errorf("expected 1 unique file hash, got %d", len(snap.Files))
	}
}

func TestCapture_ExcludePatterns(t *testing.T) {
	tmpDir := t.TempDir()

	_ = os.MkdirAll(filepath.Join(tmpDir, "node_modules", "pkg"), 0755)
	_ = os.WriteFile(filepath.Join(tmpDir, "main.js"), []byte("console.log('hi')"), 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "debug.log"), []byte("debug info"), 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "node_modules", "pkg", "index.js"), []byte("module"), 0644)

	snap, err := Capture(tmpDir, WithExclude("*.log", "node_modules"))
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}

	files, _ := snap.ListFiles()
	if len(files) != 1 {
		t.Errorf("expected 1 file after exclusions, got %d: %v", len(files), files)
	}
}

func TestCapture_Symlinks(t *testing.T) {
	tmpDir := t.TempDir()

	// Create a file and symlink to it
	_ = os.WriteFile(filepath.Join(tmpDir, "target.txt"), []byte("target content"), 0644)
	_ = os.Symlink("target.txt", filepath.Join(tmpDir, "link.txt"))

	snap, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}

	if snap.Stats.FileCount != 1 {
		t.Errorf("expected 1 file, got %d", snap.Stats.FileCount)
	}
	if snap.Stats.SymlinkCount != 1 {
		t.Errorf("expected 1 symlink, got %d", snap.Stats.SymlinkCount)
	}
}

func TestCapture_ModeBits(t *testing.T) {
	tmpDir := t.TempDir()

	// Create executable and regular file
	_ = os.WriteFile(filepath.Join(tmpDir, "script.sh"), []byte("#!/bin/bash"), 0755)
	_ = os.WriteFile(filepath.Join(tmpDir, "data.txt"), []byte("data"), 0644)

	snap, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}

	entries, err := snap.GetRootEntries()
	if err != nil {
		t.Fatalf("GetRootEntries failed: %v", err)
	}

	modes := make(map[string]uint32)
	for _, e := range entries {
		modes[e.Name] = e.Mode
	}

	if modes["script.sh"] != 0755 {
		t.Errorf("script.sh mode: expected 0755, got %o", modes["script.sh"])
	}
	if modes["data.txt"] != 0644 {
		t.Errorf("data.txt mode: expected 0644, got %o", modes["data.txt"])
	}
}

func TestSnapshot_Diff(t *testing.T) {
	tmpDir := t.TempDir()

	// Initial state
	_ = os.WriteFile(filepath.Join(tmpDir, "keep.txt"), []byte("keep"), 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "modify.txt"), []byte("original"), 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "delete.txt"), []byte("delete me"), 0644)

	snap1, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture 1 failed: %v", err)
	}

	// Modify state
	os.WriteFile(filepath.Join(tmpDir, "modify.txt"), []byte("modified"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "new.txt"), []byte("new file"), 0644)
	os.Remove(filepath.Join(tmpDir, "delete.txt"))

	snap2, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture 2 failed: %v", err)
	}

	diff, err := snap2.Diff(snap1)
	if err != nil {
		t.Fatalf("Diff failed: %v", err)
	}

	if len(diff.Added) != 1 || diff.Added[0] != "new.txt" {
		t.Errorf("expected [new.txt] added, got %v", diff.Added)
	}
	if len(diff.Modified) != 1 || diff.Modified[0] != "modify.txt" {
		t.Errorf("expected [modify.txt] modified, got %v", diff.Modified)
	}
	if len(diff.Removed) != 1 || diff.Removed[0] != "delete.txt" {
		t.Errorf("expected [delete.txt] removed, got %v", diff.Removed)
	}
}

func TestSnapshot_GetFileAtPath(t *testing.T) {
	tmpDir := t.TempDir()

	_ = os.MkdirAll(filepath.Join(tmpDir, "src", "pkg"), 0755)
	_ = os.WriteFile(filepath.Join(tmpDir, "src", "pkg", "main.go"), []byte("package main"), 0644)

	snap, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}

	entry, reader, err := snap.GetFileAtPath("src/pkg/main.go")
	if err != nil {
		t.Fatalf("GetFileAtPath failed: %v", err)
	}
	defer reader.Close()

	if entry.Name != "main.go" {
		t.Errorf("expected name 'main.go', got '%s'", entry.Name)
	}
	if entry.Kind != EntryKindFile {
		t.Errorf("expected file kind, got %d", entry.Kind)
	}
}

func TestTracker_SnapshotIfChanged(t *testing.T) {
	tmpDir := t.TempDir()
	os.WriteFile(filepath.Join(tmpDir, "file.txt"), []byte("content"), 0644)

	tracker := NewTracker(tmpDir)

	// First snapshot should always return
	snap1, changed, err := tracker.SnapshotIfChanged()
	if err != nil {
		t.Fatalf("First snapshot failed: %v", err)
	}
	if snap1 == nil || !changed {
		t.Error("first snapshot should return non-nil and changed=true")
	}

	// Second snapshot without changes should detect no change
	snap2, changed, err := tracker.SnapshotIfChanged()
	if err != nil {
		t.Fatalf("Second snapshot failed: %v", err)
	}
	if changed {
		t.Error("second snapshot should detect no changes")
	}
	if snap2 != nil {
		t.Error("snapshot should be nil when unchanged")
	}

	// Modify file
	os.WriteFile(filepath.Join(tmpDir, "file.txt"), []byte("modified"), 0644)

	// Third snapshot should detect change
	snap3, changed, err := tracker.SnapshotIfChanged()
	if err != nil {
		t.Fatalf("Third snapshot failed: %v", err)
	}
	if !changed || snap3 == nil {
		t.Error("third snapshot should detect changes")
	}
}

func TestCapture_EmptyDirectory(t *testing.T) {
	tmpDir := t.TempDir()

	snap, err := Capture(tmpDir)
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}

	if snap.Stats.FileCount != 0 {
		t.Errorf("expected 0 files, got %d", snap.Stats.FileCount)
	}
	if snap.Stats.DirCount != 1 { // just root
		t.Errorf("expected 1 directory (root), got %d", snap.Stats.DirCount)
	}
}

func TestCapture_MaxFileSize(t *testing.T) {
	tmpDir := t.TempDir()

	// Create a file larger than max
	largeContent := make([]byte, 1024) // 1KB
	os.WriteFile(filepath.Join(tmpDir, "large.bin"), largeContent, 0644)
	os.WriteFile(filepath.Join(tmpDir, "small.txt"), []byte("small"), 0644)

	snap, err := Capture(tmpDir, WithMaxFileSize(100)) // 100 byte max
	if err != nil {
		t.Fatalf("Capture failed: %v", err)
	}

	// Large file should be skipped
	if snap.Stats.FileCount != 1 {
		t.Errorf("expected 1 file (small only), got %d", snap.Stats.FileCount)
	}
}
