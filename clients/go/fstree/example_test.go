// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package fstree_test

import (
	"fmt"
	"log"
	"os"
	"path/filepath"

	"github.com/strongdm/ai-cxdb/clients/go/fstree"
)

func Example() {
	// Create a temp workspace
	tmpDir, _ := os.MkdirTemp("", "workspace")
	defer os.RemoveAll(tmpDir)

	_ = os.MkdirAll(filepath.Join(tmpDir, "src"), 0755)
	_ = os.WriteFile(filepath.Join(tmpDir, "README.md"), []byte("# My Project"), 0644)
	_ = os.WriteFile(filepath.Join(tmpDir, "src", "main.go"), []byte("package main"), 0644)

	// Capture filesystem snapshot
	snap, err := fstree.Capture(tmpDir,
		fstree.WithExclude(".git/**", "node_modules/**", "*.log"),
	)
	if err != nil {
		log.Fatal(err)
	}

	fmt.Printf("Root hash: %x\n", snap.RootHash[:8])
	fmt.Printf("Files: %d, Dirs: %d\n", snap.Stats.FileCount, snap.Stats.DirCount)
	fmt.Printf("Total bytes: %d\n", snap.Stats.TotalBytes)
	fmt.Printf("Duration: %v\n", snap.Stats.Duration)

	// List all files
	files, _ := snap.ListFiles()
	for _, f := range files {
		fmt.Println("  ", f)
	}
}

func Example_tracker() {
	tmpDir, _ := os.MkdirTemp("", "workspace")
	defer os.RemoveAll(tmpDir)
	os.WriteFile(filepath.Join(tmpDir, "file.txt"), []byte("v1"), 0644)

	// Create tracker for incremental snapshots
	tracker := fstree.NewTracker(tmpDir)

	// First snapshot
	snap1, changed, _ := tracker.SnapshotIfChanged()
	fmt.Printf("Snapshot 1: changed=%v, root=%x\n", changed, snap1.RootHash[:8])

	// No changes - returns nil
	snap2, changed, _ := tracker.SnapshotIfChanged()
	fmt.Printf("Snapshot 2: changed=%v, snap=%v\n", changed, snap2 == nil)

	// Modify file
	os.WriteFile(filepath.Join(tmpDir, "file.txt"), []byte("v2"), 0644)

	// Detects change
	snap3, changed, _ := tracker.SnapshotIfChanged()
	fmt.Printf("Snapshot 3: changed=%v, root=%x\n", changed, snap3.RootHash[:8])
}

func Example_diff() {
	tmpDir, _ := os.MkdirTemp("", "workspace")
	defer os.RemoveAll(tmpDir)

	// Initial state
	os.WriteFile(filepath.Join(tmpDir, "keep.txt"), []byte("keep"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "modify.txt"), []byte("v1"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "delete.txt"), []byte("bye"), 0644)

	snap1, _ := fstree.Capture(tmpDir)

	// Make changes
	os.WriteFile(filepath.Join(tmpDir, "modify.txt"), []byte("v2"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "new.txt"), []byte("hello"), 0644)
	os.Remove(filepath.Join(tmpDir, "delete.txt"))

	snap2, _ := fstree.Capture(tmpDir)

	// Compute diff
	diff, _ := snap2.Diff(snap1)

	fmt.Printf("Added: %v\n", diff.Added)
	fmt.Printf("Modified: %v\n", diff.Modified)
	fmt.Printf("Removed: %v\n", diff.Removed)
}
