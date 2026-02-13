// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package cxdb

import (
	"context"
	"encoding/json"
	"reflect"
	"sync"
	"testing"
)

type stubTurnClient struct {
	mu    sync.Mutex
	turns map[uint64][]TurnRecord
	heads map[uint64]*ContextHead
}

func newStubTurnClient() *stubTurnClient {
	return &stubTurnClient{
		turns: make(map[uint64][]TurnRecord),
		heads: make(map[uint64]*ContextHead),
	}
}

func (s *stubTurnClient) setContext(contextID uint64, turns []TurnRecord) {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.turns[contextID] = append([]TurnRecord{}, turns...)
	if len(turns) == 0 {
		s.heads[contextID] = &ContextHead{ContextID: contextID, HeadTurnID: 0, HeadDepth: 0}
		return
	}
	head := turns[len(turns)-1]
	s.heads[contextID] = &ContextHead{ContextID: contextID, HeadTurnID: head.TurnID, HeadDepth: head.Depth}
}

func (s *stubTurnClient) GetHead(ctx context.Context, contextID uint64) (*ContextHead, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	head, ok := s.heads[contextID]
	if !ok {
		return nil, ErrContextNotFound
	}
	copy := *head
	return &copy, nil
}

func (s *stubTurnClient) GetLast(ctx context.Context, contextID uint64, opts GetLastOptions) ([]TurnRecord, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	turns, ok := s.turns[contextID]
	if !ok {
		return nil, ErrContextNotFound
	}
	limit := int(opts.Limit)
	if limit <= 0 || limit > len(turns) {
		limit = len(turns)
	}
	start := len(turns) - limit
	result := append([]TurnRecord{}, turns[start:]...)
	return result, nil
}

func TestFollowTurnsBackfillAndDedupe(t *testing.T) {
	t.Parallel()

	client := newStubTurnClient()
	contextID := uint64(1)
	client.setContext(contextID, []TurnRecord{
		{TurnID: 1, Depth: 0},
		{TurnID: 2, Depth: 1},
	})

	events := make(chan Event, 10)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	out, errs := FollowTurns(ctx, events, client, WithFollowBuffer(10))

	events <- makeTurnEvent(contextID, 2, 1)

	client.setContext(contextID, []TurnRecord{
		{TurnID: 1, Depth: 0},
		{TurnID: 2, Depth: 1},
		{TurnID: 3, Depth: 2},
	})
	events <- makeTurnEvent(contextID, 3, 2)
	events <- makeTurnEvent(contextID, 3, 2)
	close(events)

	var got []uint64
	for turn := range out {
		got = append(got, turn.Turn.TurnID)
	}
	for err := range errs {
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
	}

	want := []uint64{1, 2, 3}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("unexpected turns: got %v want %v", got, want)
	}
}

func TestFollowTurnsOutOfOrder(t *testing.T) {
	t.Parallel()

	client := newStubTurnClient()
	contextID := uint64(2)
	client.setContext(contextID, []TurnRecord{
		{TurnID: 10, Depth: 0},
		{TurnID: 11, Depth: 1},
	})

	events := make(chan Event, 10)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	out, errs := FollowTurns(ctx, events, client, WithFollowBuffer(10))

	events <- makeTurnEvent(contextID, 11, 1)
	events <- makeTurnEvent(contextID, 10, 0)
	close(events)

	var got []uint64
	for turn := range out {
		got = append(got, turn.Turn.TurnID)
	}
	for err := range errs {
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
	}

	want := []uint64{10, 11}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("unexpected turns: got %v want %v", got, want)
	}
}

func TestFollowTurnsMultipleContexts(t *testing.T) {
	t.Parallel()

	client := newStubTurnClient()
	client.setContext(1, []TurnRecord{
		{TurnID: 1, Depth: 0},
		{TurnID: 2, Depth: 1},
	})
	client.setContext(2, []TurnRecord{
		{TurnID: 10, Depth: 0},
	})

	events := make(chan Event, 10)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	out, errs := FollowTurns(ctx, events, client, WithFollowBuffer(10))

	events <- makeTurnEvent(2, 10, 0)
	events <- makeTurnEvent(1, 2, 1)
	close(events)

	var got []uint64
	for turn := range out {
		got = append(got, turn.Turn.TurnID)
	}
	for err := range errs {
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
	}

	if len(got) != 3 {
		t.Fatalf("unexpected turns: got %v", got)
	}
}

func makeTurnEvent(contextID, turnID uint64, depth uint32) Event {
	payload := map[string]any{
		"context_id":     contextID,
		"turn_id":        turnID,
		"parent_turn_id": 0,
		"depth":          depth,
	}
	data, _ := json.Marshal(payload)
	return Event{Type: "turn_appended", Data: data}
}
