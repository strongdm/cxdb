// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	cxdb "github.com/strongdm/ai-cxdb/clients/go"
	"github.com/strongdm/ai-cxdb/clients/go/types"
)

type eventOutput struct {
	Kind string          `json:"kind"`
	Type string          `json:"type"`
	Data json.RawMessage `json:"data"`
}

type turnOutput struct {
	Kind            string                  `json:"kind"`
	ContextID       uint64                  `json:"context_id"`
	TurnID          uint64                  `json:"turn_id"`
	Depth           uint32                  `json:"depth"`
	DeclaredTypeID  string                  `json:"declared_type_id,omitempty"`
	DeclaredTypeVer uint32                  `json:"declared_type_version,omitempty"`
	Item            *types.ConversationItem `json:"item,omitempty"`
	DecodeError     string                  `json:"decode_error,omitempty"`
}

func main() {
	var (
		eventsURL string
		binAddr   string
		follow    bool
		useTLS    bool
		clientTag string
		maxEvents int
		maxTurns  int
		maxErrors int
	)

	flag.StringVar(&eventsURL, "cxdb-events-url", "", "CXDB SSE events URL (required)")
	flag.StringVar(&binAddr, "cxdb-bin-addr", "", "CXDB binary address (required for --follow-turns)")
	flag.BoolVar(&follow, "follow-turns", false, "Follow turns via binary protocol")
	flag.BoolVar(&useTLS, "tls", false, "Use TLS for binary protocol")
	flag.StringVar(&clientTag, "client-tag", "", "Optional client tag for binary protocol")
	flag.IntVar(&maxEvents, "max-events", 0, "Stop after N SSE events (0 = no limit)")
	flag.IntVar(&maxTurns, "max-turns", 0, "Stop after N decoded turns (0 = no limit)")
	flag.IntVar(&maxErrors, "max-errors", 0, "Stop after N errors (0 = no limit)")
	flag.Parse()

	if eventsURL == "" {
		fmt.Fprintln(os.Stderr, "--cxdb-events-url is required")
		os.Exit(2)
	}
	if follow && binAddr == "" {
		fmt.Fprintln(os.Stderr, "--cxdb-bin-addr is required when --follow-turns is set")
		os.Exit(2)
	}

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	events, errs := cxdb.SubscribeEvents(ctx, eventsURL)

	var client *cxdb.Client
	if follow {
		var err error
		if useTLS {
			client, err = cxdb.DialTLS(binAddr, cxdb.WithClientTag(clientTag))
		} else {
			client, err = cxdb.Dial(binAddr, cxdb.WithClientTag(clientTag))
		}
		if err != nil {
			fmt.Fprintf(os.Stderr, "dial cxdb: %v\n", err)
			os.Exit(1)
		}
		defer func() {
			_ = client.Close()
		}()
	}

	eventOut := events
	var followEvents <-chan cxdb.Event

	if follow {
		teeOut := make(chan cxdb.Event, 128)
		teeFollow := make(chan cxdb.Event, 128)
		followEvents = teeFollow
		eventOut = teeOut

		go func() {
			defer close(teeOut)
			defer close(teeFollow)
			for ev := range events {
				select {
				case <-ctx.Done():
					return
				case teeOut <- ev:
				}
				select {
				case <-ctx.Done():
					return
				case teeFollow <- ev:
				}
			}
		}()

		turns, turnErrs := cxdb.FollowTurns(ctx, followEvents, client)
		errorCount := consume(ctx, cancel, eventOut, errs, turnErrs, turns, maxEvents, maxTurns, maxErrors)
		if maxErrors > 0 && errorCount >= maxErrors {
			os.Exit(1)
		}
		return
	}

	errorCount := consume(ctx, cancel, eventOut, errs, nil, nil, maxEvents, maxTurns, maxErrors)
	if maxErrors > 0 && errorCount >= maxErrors {
		os.Exit(1)
	}
}

func consume(
	ctx context.Context,
	cancel context.CancelFunc,
	events <-chan cxdb.Event,
	errs <-chan error,
	turnErrs <-chan error,
	turns <-chan cxdb.FollowTurn,
	maxEvents int,
	maxTurns int,
	maxErrors int,
) int {
	eventCount := 0
	turnCount := 0
	errorCount := 0

	stopIfDone := func() {
		stopOnEvents := maxEvents > 0
		stopOnTurns := maxTurns > 0
		stopOnErrors := maxErrors > 0
		if stopOnErrors && errorCount >= maxErrors {
			cancel()
			return
		}
		if (stopOnEvents && eventCount >= maxEvents) || (stopOnTurns && turnCount >= maxTurns) {
			if !stopOnEvents || eventCount >= maxEvents {
				if !stopOnTurns || turnCount >= maxTurns {
					cancel()
				}
			}
		}
	}

	for {
		select {
		case <-ctx.Done():
			return errorCount
		case ev, ok := <-events:
			if !ok {
				events = nil
				break
			}
			printEvent(ev)
			eventCount++
			stopIfDone()
		case err, ok := <-errs:
			if !ok {
				errs = nil
				break
			}
			if err != nil {
				fmt.Fprintf(os.Stderr, "subscribe error: %v\n", err)
				errorCount++
				stopIfDone()
			}
		case err, ok := <-turnErrs:
			if !ok {
				turnErrs = nil
				break
			}
			if err != nil {
				fmt.Fprintf(os.Stderr, "follow error: %v\n", err)
				errorCount++
				stopIfDone()
			}
		case turn, ok := <-turns:
			if !ok {
				turns = nil
				break
			}
			printTurn(turn)
			turnCount++
			stopIfDone()
		}

		if events == nil && errs == nil && turns == nil && turnErrs == nil {
			return errorCount
		}
	}
}

func printEvent(ev cxdb.Event) {
	out := eventOutput{Kind: "event", Type: ev.Type, Data: ev.Data}
	data, err := json.Marshal(out)
	if err != nil {
		fmt.Fprintf(os.Stderr, "encode event: %v\n", err)
		return
	}
	_, _ = fmt.Fprintln(os.Stdout, string(data))
}

func printTurn(turn cxdb.FollowTurn) {
	result := turnOutput{
		Kind:            "turn",
		ContextID:       turn.ContextID,
		TurnID:          turn.Turn.TurnID,
		Depth:           turn.Turn.Depth,
		DeclaredTypeID:  turn.Turn.TypeID,
		DeclaredTypeVer: turn.Turn.TypeVersion,
	}

	if turn.Turn.Encoding != cxdb.EncodingMsgpack {
		result.DecodeError = "unsupported encoding"
	} else if turn.Turn.Compression != cxdb.CompressionNone {
		result.DecodeError = "unsupported compression"
	} else {
		var item types.ConversationItem
		if err := cxdb.DecodeMsgpackInto(turn.Turn.Payload, &item); err != nil {
			result.DecodeError = err.Error()
		} else {
			result.Item = &item
		}
	}

	data, err := json.Marshal(result)
	if err != nil {
		fmt.Fprintf(os.Stderr, "encode turn: %v\n", err)
		return
	}
	_, _ = fmt.Fprintln(os.Stdout, string(data))
}
