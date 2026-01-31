// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package proxy

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"sync"
	"time"
)

// SSEBroker manages SSE connections and broadcasts events to all connected clients.
type SSEBroker struct {
	mu      sync.RWMutex
	clients map[chan []byte]struct{}
	logger  *slog.Logger

	// Polling state
	backend       string
	pollInterval  time.Duration
	lastContexts  map[string]contextState // context_id -> state
	lastPollError error
}

type contextState struct {
	HeadTurnID string `json:"head_turn_id"`
	HeadDepth  int    `json:"head_depth"`
}

type contextsResponse struct {
	Contexts []struct {
		ContextID       string `json:"context_id"`
		HeadTurnID      string `json:"head_turn_id"`
		HeadDepth       int    `json:"head_depth"`
		CreatedAtUnixMs int64  `json:"created_at_unix_ms"`
	} `json:"contexts"`
}

// NewSSEBroker creates a new SSE broker that polls the backend for changes.
func NewSSEBroker(backendURL string, logger *slog.Logger) *SSEBroker {
	return &SSEBroker{
		clients:      make(map[chan []byte]struct{}),
		logger:       logger,
		backend:      backendURL,
		pollInterval: 2 * time.Second,
		lastContexts: make(map[string]contextState),
	}
}

// Start begins polling the backend for changes.
func (b *SSEBroker) Start(ctx context.Context) {
	go b.pollLoop(ctx)
}

func (b *SSEBroker) pollLoop(ctx context.Context) {
	ticker := time.NewTicker(b.pollInterval)
	defer ticker.Stop()

	// Initial poll
	b.poll()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			b.poll()
		}
	}
}

func (b *SSEBroker) poll() {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, "GET", b.backend+"/v1/contexts?limit=50", nil)
	if err != nil {
		b.lastPollError = err
		return
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		b.lastPollError = err
		return
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		b.lastPollError = fmt.Errorf("backend returned %d", resp.StatusCode)
		return
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		b.lastPollError = err
		return
	}

	var data contextsResponse
	if err := json.Unmarshal(body, &data); err != nil {
		b.lastPollError = err
		return
	}

	b.lastPollError = nil

	// Check for new/updated contexts
	newContexts := make(map[string]contextState)
	for _, ctx := range data.Contexts {
		newContexts[ctx.ContextID] = contextState{
			HeadTurnID: ctx.HeadTurnID,
			HeadDepth:  ctx.HeadDepth,
		}

		oldState, exists := b.lastContexts[ctx.ContextID]
		if !exists {
			// New context
			b.broadcast(Event{
				Type: "context_created",
				Data: map[string]interface{}{
					"context_id": ctx.ContextID,
					"created_at": ctx.CreatedAtUnixMs,
				},
			})
		} else if oldState.HeadTurnID != ctx.HeadTurnID {
			// Turn appended
			b.broadcast(Event{
				Type: "turn_appended",
				Data: map[string]interface{}{
					"context_id":     ctx.ContextID,
					"turn_id":        ctx.HeadTurnID,
					"parent_turn_id": oldState.HeadTurnID,
					"depth":          ctx.HeadDepth,
				},
			})
		}
	}

	b.lastContexts = newContexts
}

// Event represents an SSE event to broadcast.
type Event struct {
	Type string                 `json:"type"`
	Data map[string]interface{} `json:"data"`
}

func (b *SSEBroker) broadcast(event Event) {
	data, err := json.Marshal(event.Data)
	if err != nil {
		b.logger.Error("failed to marshal event", "err", err)
		return
	}

	// Format as SSE: event: <type>\ndata: <json>\n\n
	msg := []byte(fmt.Sprintf("event: %s\ndata: %s\n\n", event.Type, data))

	b.mu.RLock()
	defer b.mu.RUnlock()

	for ch := range b.clients {
		select {
		case ch <- msg:
		default:
			// Client buffer full, skip
		}
	}
}

// Subscribe adds a client to receive events.
func (b *SSEBroker) Subscribe() chan []byte {
	ch := make(chan []byte, 10)
	b.mu.Lock()
	b.clients[ch] = struct{}{}
	b.mu.Unlock()
	b.logger.Info("sse_client_connected", "total_clients", len(b.clients))
	return ch
}

// Unsubscribe removes a client.
func (b *SSEBroker) Unsubscribe(ch chan []byte) {
	b.mu.Lock()
	delete(b.clients, ch)
	close(ch)
	b.mu.Unlock()
	b.logger.Info("sse_client_disconnected", "total_clients", len(b.clients))
}

// ServeHTTP handles SSE connections at /v1/events.
func (b *SSEBroker) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	// Check if client supports SSE
	flusher, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "SSE not supported", http.StatusInternalServerError)
		return
	}

	// Set SSE headers - must be set before any writes
	// Note: Do NOT set Connection header - it's invalid in HTTP/2 and browsers reject it
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("X-Accel-Buffering", "no") // Disable nginx buffering
	// Add explicit CORS headers for browser compatibility
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.Header().Set("Access-Control-Allow-Credentials", "true")

	// Subscribe to events
	ch := b.Subscribe()
	defer b.Unsubscribe(ch)

	// Send minimal initial message - just retry and comment
	b.logger.Info("sse_sending_connected")
	_, _ = fmt.Fprintf(w, "retry: 10000\n\n")
	flusher.Flush()

	// Send connected event
	_, _ = fmt.Fprintf(w, "event: connected\ndata: {\"status\":\"connected\"}\n\n")
	flusher.Flush()
	b.logger.Info("sse_flushed_connected")

	// Keep-alive ticker to prevent ALB/proxy timeouts
	keepAlive := time.NewTicker(5 * time.Second)
	defer keepAlive.Stop()

	// Stream events until client disconnects
	b.logger.Info("sse_entering_loop")
	for {
		select {
		case <-r.Context().Done():
			b.logger.Info("sse_context_done", "err", r.Context().Err())
			return
		case <-keepAlive.C:
			// Send SSE comment as keep-alive (: comment\n\n)
			_, err := fmt.Fprintf(w, ": keepalive %d\n\n", time.Now().Unix())
			if err != nil {
				return
			}
			flusher.Flush()
		case msg := <-ch:
			_, err := w.Write(msg)
			if err != nil {
				return
			}
			flusher.Flush()
		}
	}
}

// ClientCount returns the number of connected SSE clients.
func (b *SSEBroker) ClientCount() int {
	b.mu.RLock()
	defer b.mu.RUnlock()
	return len(b.clients)
}
