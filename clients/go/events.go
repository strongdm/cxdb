// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package cxdb

import (
	"encoding/json"
)

// ContextCreatedEvent represents a context_created SSE event payload.
type ContextCreatedEvent struct {
	ContextID uint64
	SessionID string
	ClientTag string
	CreatedAt int64
}

// ContextMetadataUpdatedEvent represents a context_metadata_updated SSE event payload.
type ContextMetadataUpdatedEvent struct {
	ContextID     uint64
	HasProvenance bool
	ClientTag     string
	Title         string
	Labels        []string
}

// TurnAppendedEvent represents a turn_appended SSE event payload.
type TurnAppendedEvent struct {
	ContextID           uint64
	TurnID              uint64
	ParentTurnID        uint64
	Depth               uint32
	DeclaredTypeID      string
	DeclaredTypeVersion uint32
	HasDeclaredTypeID   bool
	HasDeclaredTypeVer  bool
}

// ClientConnectedEvent represents a client_connected SSE event payload.
type ClientConnectedEvent struct {
	SessionID string
	ClientTag string
}

// ClientDisconnectedEvent represents a client_disconnected SSE event payload.
type ClientDisconnectedEvent struct {
	SessionID string
	ClientTag string
	Contexts  []string
}

type contextCreatedPayload struct {
	ContextID sseUint64 `json:"context_id"`
	SessionID string    `json:"session_id"`
	ClientTag string    `json:"client_tag"`
	CreatedAt sseInt64  `json:"created_at"`
}

type contextMetadataUpdatedPayload struct {
	ContextID     sseUint64 `json:"context_id"`
	HasProvenance bool      `json:"has_provenance"`
	ClientTag     string    `json:"client_tag"`
	Title         string    `json:"title"`
	Labels        []string  `json:"labels"`
}

type turnAppendedPayload struct {
	ContextID       sseUint64  `json:"context_id"`
	TurnID          sseUint64  `json:"turn_id"`
	ParentTurnID    sseUint64  `json:"parent_turn_id"`
	Depth           sseUint32  `json:"depth"`
	DeclaredTypeID  string     `json:"declared_type_id"`
	DeclaredTypeVer *sseUint32 `json:"declared_type_version"`
}

type clientConnectedPayload struct {
	SessionID string `json:"session_id"`
	ClientTag string `json:"client_tag"`
}

type clientDisconnectedPayload struct {
	SessionID string   `json:"session_id"`
	ClientTag string   `json:"client_tag"`
	Contexts  []string `json:"contexts"`
}

// DecodeContextCreated decodes a context_created payload into a typed event.
func DecodeContextCreated(data json.RawMessage) (ContextCreatedEvent, error) {
	var payload contextCreatedPayload
	if err := json.Unmarshal(data, &payload); err != nil {
		return ContextCreatedEvent{}, err
	}
	return ContextCreatedEvent{
		ContextID: payload.ContextID.Value,
		SessionID: payload.SessionID,
		ClientTag: payload.ClientTag,
		CreatedAt: payload.CreatedAt.Value,
	}, nil
}

// DecodeContextMetadataUpdated decodes a context_metadata_updated payload into a typed event.
func DecodeContextMetadataUpdated(data json.RawMessage) (ContextMetadataUpdatedEvent, error) {
	var payload contextMetadataUpdatedPayload
	if err := json.Unmarshal(data, &payload); err != nil {
		return ContextMetadataUpdatedEvent{}, err
	}
	return ContextMetadataUpdatedEvent{
		ContextID:     payload.ContextID.Value,
		HasProvenance: payload.HasProvenance,
		ClientTag:     payload.ClientTag,
		Title:         payload.Title,
		Labels:        payload.Labels,
	}, nil
}

// DecodeTurnAppended decodes a turn_appended payload into a typed event.
func DecodeTurnAppended(data json.RawMessage) (TurnAppendedEvent, error) {
	var payload turnAppendedPayload
	if err := json.Unmarshal(data, &payload); err != nil {
		return TurnAppendedEvent{}, err
	}
	event := TurnAppendedEvent{
		ContextID:      payload.ContextID.Value,
		TurnID:         payload.TurnID.Value,
		ParentTurnID:   payload.ParentTurnID.Value,
		Depth:          payload.Depth.Value,
		DeclaredTypeID: payload.DeclaredTypeID,
	}
	if payload.DeclaredTypeID != "" {
		event.HasDeclaredTypeID = true
	}
	if payload.DeclaredTypeVer != nil {
		event.DeclaredTypeVersion = payload.DeclaredTypeVer.Value
		event.HasDeclaredTypeVer = true
	}
	return event, nil
}

// DecodeClientConnected decodes a client_connected payload into a typed event.
func DecodeClientConnected(data json.RawMessage) (ClientConnectedEvent, error) {
	var payload clientConnectedPayload
	if err := json.Unmarshal(data, &payload); err != nil {
		return ClientConnectedEvent{}, err
	}
	return ClientConnectedEvent(payload), nil
}

// DecodeClientDisconnected decodes a client_disconnected payload into a typed event.
func DecodeClientDisconnected(data json.RawMessage) (ClientDisconnectedEvent, error) {
	var payload clientDisconnectedPayload
	if err := json.Unmarshal(data, &payload); err != nil {
		return ClientDisconnectedEvent{}, err
	}
	return ClientDisconnectedEvent(payload), nil
}
