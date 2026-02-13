// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package cxdb

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestReadEventStreamMultiLine(t *testing.T) {
	t.Parallel()

	input := "event: turn_appended\n" +
		"data: {\"a\":1}\n" +
		"data: {\"b\":2}\n\n"

	var events []Event
	err := readEventStream(context.Background(), strings.NewReader(input), 1024, func(ev Event) error {
		events = append(events, ev)
		return nil
	})
	if !errors.Is(err, io.EOF) {
		t.Fatalf("expected EOF, got %v", err)
	}
	if len(events) != 1 {
		t.Fatalf("expected 1 event, got %d", len(events))
	}
	if events[0].Type != "turn_appended" {
		t.Fatalf("expected type turn_appended, got %q", events[0].Type)
	}
	want := "{\"a\":1}\n{\"b\":2}"
	if string(events[0].Data) != want {
		t.Fatalf("unexpected data: %s", string(events[0].Data))
	}
}

func TestReadEventStreamDefaultTypeAndComments(t *testing.T) {
	t.Parallel()

	input := ": heartbeat\n" +
		"data: {\"ok\":true}\n\n"

	var events []Event
	err := readEventStream(context.Background(), strings.NewReader(input), 1024, func(ev Event) error {
		events = append(events, ev)
		return nil
	})
	if !errors.Is(err, io.EOF) {
		t.Fatalf("expected EOF, got %v", err)
	}
	if len(events) != 1 {
		t.Fatalf("expected 1 event, got %d", len(events))
	}
	if events[0].Type != "message" {
		t.Fatalf("expected default type message, got %q", events[0].Type)
	}
	if string(events[0].Data) != "{\"ok\":true}" {
		t.Fatalf("unexpected data: %s", string(events[0].Data))
	}
}

func TestReadEventStreamOversize(t *testing.T) {
	t.Parallel()

	input := "event: big\n" +
		"data: " + strings.Repeat("x", 20) + "\n\n"

	err := readEventStream(context.Background(), strings.NewReader(input), 10, func(ev Event) error {
		return nil
	})
	if err == nil {
		t.Fatal("expected error for oversize event")
	}
}

func TestReadEventStreamMalformedField(t *testing.T) {
	t.Parallel()

	input := "bad field\n\n"
	err := readEventStream(context.Background(), strings.NewReader(input), 1024, func(ev Event) error {
		return nil
	})
	if err == nil {
		t.Fatal("expected error for malformed field")
	}
}

func TestSubscribeEventsReconnect(t *testing.T) {
	t.Parallel()

	var connections int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		flusher, ok := w.(http.Flusher)
		if !ok {
			return
		}
		switch atomic.AddInt32(&connections, 1) {
		case 1:
			_, _ = w.Write([]byte("event: turn_appended\n"))
			_, _ = w.Write([]byte("data: {\"context_id\":\"1\",\"turn_id\":\"1\",\"parent_turn_id\":\"0\",\"depth\":0}\n\n"))
			flusher.Flush()
		case 2:
			_, _ = w.Write([]byte("event: turn_appended\n"))
			_, _ = w.Write([]byte("data: {\"context_id\":\"1\",\"turn_id\":\"2\",\"parent_turn_id\":\"1\",\"depth\":1}\n\n"))
			flusher.Flush()
		default:
			return
		}
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	events, _ := SubscribeEvents(ctx, srv.URL,
		WithSubscribeRetryDelay(5*time.Millisecond),
		WithSubscribeMaxRetryDelay(20*time.Millisecond),
	)

	var got []Event
	deadline := time.After(2 * time.Second)
	for len(got) < 2 {
		select {
		case ev := <-events:
			if ev.Type != "" {
				got = append(got, ev)
			}
		case <-deadline:
			t.Fatalf("timed out waiting for events, got %d", len(got))
		}
	}

	cancel()
	if got[0].Type != "turn_appended" || got[1].Type != "turn_appended" {
		t.Fatalf("unexpected event types: %#v", got)
	}
	var payload map[string]any
	if err := json.Unmarshal(got[0].Data, &payload); err != nil {
		t.Fatalf("decode event data: %v", err)
	}
}

func TestSubscribeEventsInvalidURL(t *testing.T) {
	t.Parallel()

	events, errs := SubscribeEvents(context.Background(), "")
	if _, ok := <-events; ok {
		t.Fatal("expected events channel to close")
	}
	err := <-errs
	if err == nil {
		t.Fatal("expected error")
	}
}

func TestSubscribeEventsHeadersAndCancel(t *testing.T) {
	t.Parallel()

	var sawHeader int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("X-Test-Header") == "ok" {
			atomic.StoreInt32(&sawHeader, 1)
		}
		w.Header().Set("Content-Type", "text/event-stream")
		flusher, ok := w.(http.Flusher)
		if !ok {
			return
		}
		_, _ = w.Write([]byte("data: {\"ok\":true}\n\n"))
		flusher.Flush()
	}))
	defer srv.Close()

	ctx, cancel := context.WithCancel(context.Background())
	events, errs := SubscribeEvents(ctx, srv.URL, WithHeaders(http.Header{"X-Test-Header": []string{"ok"}}))

	select {
	case <-events:
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for event")
	}

	cancel()

	select {
	case <-errs:
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for error channel close")
	}

	if atomic.LoadInt32(&sawHeader) == 0 {
		t.Fatal("expected header to be passed")
	}
}
