// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package cxdb

import (
	"encoding/json"
	"testing"
)

func TestDecodeContextCreated(t *testing.T) {
	t.Parallel()

	input := json.RawMessage(`{"context_id":"42","session_id":"sess-abc","client_tag":"ai-staff","created_at":1739481600000}`)
	ev, err := DecodeContextCreated(input)
	if err != nil {
		t.Fatalf("DecodeContextCreated: %v", err)
	}
	if ev.ContextID != 42 {
		t.Fatalf("ContextID = %d, want 42", ev.ContextID)
	}
	if ev.SessionID != "sess-abc" {
		t.Fatalf("SessionID = %q, want sess-abc", ev.SessionID)
	}
	if ev.ClientTag != "ai-staff" {
		t.Fatalf("ClientTag = %q, want ai-staff", ev.ClientTag)
	}
	if ev.CreatedAt != 1739481600000 {
		t.Fatalf("CreatedAt = %d, want 1739481600000", ev.CreatedAt)
	}
}

func TestDecodeTurnAppendedOptionalFields(t *testing.T) {
	t.Parallel()

	input := json.RawMessage(`{"context_id":7,"turn_id":"9","parent_turn_id":"8","depth":10}`)
	ev, err := DecodeTurnAppended(input)
	if err != nil {
		t.Fatalf("DecodeTurnAppended: %v", err)
	}
	if ev.ContextID != 7 || ev.TurnID != 9 || ev.ParentTurnID != 8 || ev.Depth != 10 {
		t.Fatalf("unexpected values: %+v", ev)
	}
	if ev.HasDeclaredTypeID || ev.HasDeclaredTypeVer {
		t.Fatal("expected no declared type fields")
	}
}
