// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package cxdb

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
)

// TurnClient defines the subset of client methods needed by FollowTurns.
type TurnClient interface {
	GetHead(ctx context.Context, contextID uint64) (*ContextHead, error)
	GetLast(ctx context.Context, contextID uint64, opts GetLastOptions) ([]TurnRecord, error)
}

type followOptions struct {
	bufferSize        int
	maxSeenPerContext int
}

// FollowOption configures FollowTurns behavior.
type FollowOption func(*followOptions)

// WithFollowBuffer sets the output channel buffer size.
func WithFollowBuffer(size int) FollowOption {
	return func(o *followOptions) {
		o.bufferSize = size
	}
}

// WithMaxSeenPerContext limits the number of recently seen turn IDs stored per context.
func WithMaxSeenPerContext(limit int) FollowOption {
	return func(o *followOptions) {
		o.maxSeenPerContext = limit
	}
}

const (
	defaultFollowBuffer      = 128
	defaultMaxSeenPerContext = 2048
)

// FollowTurn combines a turn record with its context ID.
type FollowTurn struct {
	ContextID uint64
	Turn      TurnRecord
}

// FollowTurns converts turn_appended SSE hints into ordered turn streams.
func FollowTurns(ctx context.Context, events <-chan Event, client TurnClient, opts ...FollowOption) (<-chan FollowTurn, <-chan error) {
	options := followOptions{
		bufferSize:        defaultFollowBuffer,
		maxSeenPerContext: defaultMaxSeenPerContext,
	}
	for _, opt := range opts {
		opt(&options)
	}

	out := make(chan FollowTurn, options.bufferSize)
	errs := make(chan error, options.bufferSize)
	states := make(map[uint64]*followState)

	go func() {
		defer close(out)
		defer close(errs)

		for {
			select {
			case <-ctx.Done():
				return
			case ev, ok := <-events:
				if !ok {
					return
				}
				if ev.Type != "turn_appended" {
					continue
				}
				turnEvent, err := decodeTurnAppended(ev.Data)
				if err != nil {
					nonBlockingSend(errs, err)
					continue
				}
				state := states[turnEvent.ContextID]
				if state == nil {
					state = newFollowState(options.maxSeenPerContext)
					states[turnEvent.ContextID] = state
				}
				if err := state.syncContext(ctx, client, turnEvent.ContextID, out); err != nil {
					nonBlockingSend(errs, err)
				}
			}
		}
	}()

	return out, errs
}

type followState struct {
	hasLast        bool
	lastSeenTurnID uint64
	lastSeenDepth  uint32
	seen           map[uint64]struct{}
	seenOrder      []uint64
	maxSeen        int
}

func newFollowState(maxSeen int) *followState {
	if maxSeen <= 0 {
		maxSeen = defaultMaxSeenPerContext
	}
	return &followState{
		seen:    make(map[uint64]struct{}),
		maxSeen: maxSeen,
	}
}

func (s *followState) syncContext(ctx context.Context, client TurnClient, contextID uint64, out chan<- FollowTurn) error {
	head, err := client.GetHead(ctx, contextID)
	if err != nil {
		return fmt.Errorf("follow turns: get head: %w", err)
	}

	if s.hasLast && head.HeadDepth < s.lastSeenDepth {
		return fmt.Errorf("follow turns: head depth regressed (context %d)", contextID)
	}

	missing := uint32(0)
	if s.hasLast {
		if head.HeadDepth > s.lastSeenDepth {
			missing = head.HeadDepth - s.lastSeenDepth
		}
	} else {
		missing = head.HeadDepth + 1
	}

	if missing == 0 {
		return nil
	}

	turns, err := client.GetLast(ctx, contextID, GetLastOptions{Limit: missing, IncludePayload: true})
	if err != nil {
		return fmt.Errorf("follow turns: get last: %w", err)
	}

	for _, turn := range turns {
		if s.seenTurn(turn.TurnID) {
			continue
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case out <- FollowTurn{ContextID: contextID, Turn: turn}:
		}
		s.recordTurn(turn)
	}

	return nil
}

func (s *followState) seenTurn(turnID uint64) bool {
	_, ok := s.seen[turnID]
	return ok
}

func (s *followState) recordTurn(turn TurnRecord) {
	s.seen[turn.TurnID] = struct{}{}
	s.seenOrder = append(s.seenOrder, turn.TurnID)
	for len(s.seenOrder) > s.maxSeen {
		oldest := s.seenOrder[0]
		s.seenOrder = s.seenOrder[1:]
		delete(s.seen, oldest)
	}
	if !s.hasLast || turn.Depth >= s.lastSeenDepth {
		s.lastSeenDepth = turn.Depth
		s.lastSeenTurnID = turn.TurnID
		s.hasLast = true
	}
}

func decodeTurnAppended(data json.RawMessage) (TurnAppendedEvent, error) {
	if len(data) == 0 {
		return TurnAppendedEvent{}, errors.New("turn_appended: empty payload")
	}
	event, err := DecodeTurnAppended(data)
	if err != nil {
		return TurnAppendedEvent{}, fmt.Errorf("turn_appended: decode: %w", err)
	}
	if event.ContextID == 0 {
		return TurnAppendedEvent{}, errors.New("turn_appended: missing context_id")
	}
	if event.TurnID == 0 {
		return TurnAppendedEvent{}, errors.New("turn_appended: missing turn_id")
	}
	return event, nil
}
