// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package cxdb

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// Event represents a single SSE event from CXDB.
type Event struct {
	Type string
	Data json.RawMessage
	ID   string
}

const (
	defaultMaxEventBytes = 2 * 1024 * 1024
	defaultEventBuffer   = 128
	defaultErrorBuffer   = 8
	defaultRetryDelay    = 500 * time.Millisecond
	defaultMaxRetryDelay = 10 * time.Second
)

type subscribeOptions struct {
	client        *http.Client
	headers       http.Header
	maxEventBytes int
	eventBuffer   int
	errorBuffer   int
	retryDelay    time.Duration
	maxRetryDelay time.Duration
}

// SubscribeOption configures SubscribeEvents behavior.
type SubscribeOption func(*subscribeOptions)

// WithHTTPClient sets a custom HTTP client for SSE subscriptions.
func WithHTTPClient(client *http.Client) SubscribeOption {
	return func(o *subscribeOptions) {
		o.client = client
	}
}

// WithHeaders sets additional headers for the SSE request.
func WithHeaders(headers http.Header) SubscribeOption {
	return func(o *subscribeOptions) {
		o.headers = headers.Clone()
	}
}

// WithMaxEventBytes caps the maximum size of a single SSE event payload.
func WithMaxEventBytes(n int) SubscribeOption {
	return func(o *subscribeOptions) {
		o.maxEventBytes = n
	}
}

// WithEventBuffer sets the event channel buffer size.
func WithEventBuffer(n int) SubscribeOption {
	return func(o *subscribeOptions) {
		o.eventBuffer = n
	}
}

// WithErrorBuffer sets the error channel buffer size.
func WithErrorBuffer(n int) SubscribeOption {
	return func(o *subscribeOptions) {
		o.errorBuffer = n
	}
}

// WithSubscribeRetryDelay sets the initial retry delay for reconnects.
func WithSubscribeRetryDelay(d time.Duration) SubscribeOption {
	return func(o *subscribeOptions) {
		o.retryDelay = d
	}
}

// WithSubscribeMaxRetryDelay caps the retry delay for reconnects.
func WithSubscribeMaxRetryDelay(d time.Duration) SubscribeOption {
	return func(o *subscribeOptions) {
		o.maxRetryDelay = d
	}
}

// SubscribeEvents subscribes to a CXDB SSE endpoint and streams events until the context is canceled.
func SubscribeEvents(ctx context.Context, url string, opts ...SubscribeOption) (<-chan Event, <-chan error) {
	options := subscribeOptions{
		client:        http.DefaultClient,
		maxEventBytes: defaultMaxEventBytes,
		eventBuffer:   defaultEventBuffer,
		errorBuffer:   defaultErrorBuffer,
		retryDelay:    defaultRetryDelay,
		maxRetryDelay: defaultMaxRetryDelay,
	}
	for _, opt := range opts {
		opt(&options)
	}

	events := make(chan Event, options.eventBuffer)
	errs := make(chan error, options.errorBuffer)

	if strings.TrimSpace(url) == "" {
		err := fmt.Errorf("cxdb subscribe: url is required")
		errs <- err
		close(events)
		close(errs)
		return events, errs
	}

	go func() {
		defer close(events)
		defer close(errs)

		retryDelay := options.retryDelay
		for {
			if ctx.Err() != nil {
				return
			}

			err := subscribeOnce(ctx, url, options, events)
			if err != nil && !errors.Is(err, context.Canceled) {
				nonBlockingSend(errs, err)
			}

			if ctx.Err() != nil {
				return
			}

			if retryDelay <= 0 {
				retryDelay = defaultRetryDelay
			}
			if options.maxRetryDelay > 0 && retryDelay > options.maxRetryDelay {
				retryDelay = options.maxRetryDelay
			}

			timer := time.NewTimer(retryDelay)
			select {
			case <-ctx.Done():
				timer.Stop()
				return
			case <-timer.C:
			}

			retryDelay = nextRetryDelay(retryDelay, options.maxRetryDelay)
		}
	}()

	return events, errs
}

func subscribeOnce(ctx context.Context, url string, options subscribeOptions, events chan<- Event) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return fmt.Errorf("cxdb subscribe: build request: %w", err)
	}
	for key, values := range options.headers {
		for _, v := range values {
			req.Header.Add(key, v)
		}
	}

	resp, err := options.client.Do(req)
	if err != nil {
		return fmt.Errorf("cxdb subscribe: request failed: %w", err)
	}
	defer func() {
		_ = resp.Body.Close()
	}()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 1024))
		return fmt.Errorf("cxdb subscribe: unexpected status %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}

	err = readEventStream(ctx, resp.Body, options.maxEventBytes, func(ev Event) error {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case events <- ev:
			return nil
		}
	})
	if err == nil || errors.Is(err, context.Canceled) {
		return err
	}
	if errors.Is(err, io.EOF) {
		return fmt.Errorf("cxdb subscribe: stream closed")
	}
	return err
}

func readEventStream(ctx context.Context, reader io.Reader, maxEventBytes int, emit func(Event) error) error {
	br := bufio.NewReader(reader)

	reset := func() (string, []string, string, int) {
		return "", nil, "", 0
	}

	eventType, dataLines, lastID, dataSize := reset()
	flush := func() error {
		if len(dataLines) == 0 && eventType == "" && lastID == "" {
			eventType, dataLines, lastID, dataSize = reset()
			return nil
		}

		data := strings.Join(dataLines, "\n")
		if data == "" {
			eventType, dataLines, lastID, dataSize = reset()
			return nil
		}

		if eventType == "" {
			eventType = "message"
		}

		event := Event{
			Type: eventType,
			Data: json.RawMessage(data),
			ID:   lastID,
		}
		err := emit(event)
		eventType, dataLines, lastID, dataSize = reset()
		return err
	}

	for {
		if ctx.Err() != nil {
			return ctx.Err()
		}

		line, err := br.ReadString('\n')
		if err != nil && !errors.Is(err, io.EOF) {
			return err
		}

		if len(line) == 0 && errors.Is(err, io.EOF) {
			return io.EOF
		}

		line = strings.TrimRight(line, "\r\n")

		if line == "" {
			if flushErr := flush(); flushErr != nil {
				return flushErr
			}
			if errors.Is(err, io.EOF) {
				return io.EOF
			}
			continue
		}

		if strings.HasPrefix(line, ":") {
			if errors.Is(err, io.EOF) {
				return io.EOF
			}
			continue
		}

		field, value, found := strings.Cut(line, ":")
		if !found {
			field = line
			value = ""
		}
		if field == "" || strings.ContainsAny(field, " \t") {
			return fmt.Errorf("cxdb subscribe: malformed field %q", field)
		}
		value = strings.TrimPrefix(value, " ")

		switch field {
		case "event":
			eventType = value
		case "data":
			dataLines = append(dataLines, value)
			dataSize += len(value)
			if maxEventBytes > 0 && dataSize > maxEventBytes {
				return fmt.Errorf("cxdb subscribe: event exceeds max size (%d bytes)", dataSize)
			}
		case "id":
			lastID = value
		case "retry":
			// ignore
		}

		if errors.Is(err, io.EOF) {
			if flushErr := flush(); flushErr != nil {
				return flushErr
			}
			return io.EOF
		}
	}
}

func nextRetryDelay(current, max time.Duration) time.Duration {
	if current <= 0 {
		return defaultRetryDelay
	}
	next := current * 2
	if max > 0 && next > max {
		return max
	}
	return next
}

func nonBlockingSend(ch chan<- error, err error) {
	select {
	case ch <- err:
	default:
	}
}
