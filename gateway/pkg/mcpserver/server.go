// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

// Package mcpserver exposes CXDB operations through remote Streamable HTTP MCP.
package mcpserver

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	mcpauth "github.com/modelcontextprotocol/go-sdk/auth"
	"github.com/modelcontextprotocol/go-sdk/mcp"
	cxdbauth "github.com/strongdm/cxdb/gateway/pkg/auth"
	"github.com/vmihailenco/msgpack/v5"
)

const maxBackendResponse = 8 << 20

// New returns a bearer-protected, origin-protected MCP Streamable HTTP handler.
func New(backendURL, resourceMetadataURL string, verifiers []cxdbauth.BearerTokenVerifier, logger *slog.Logger) (http.Handler, error) {
	backend, err := url.Parse(strings.TrimSuffix(backendURL, "/"))
	if err != nil || backend.Scheme == "" || backend.Host == "" {
		return nil, errors.New("invalid CXDB backend URL")
	}
	api := &backendClient{base: backend, client: &http.Client{Timeout: 30 * time.Second}}
	server := mcp.NewServer(&mcp.Implementation{Name: "cxdb", Version: "0.1.0"}, &mcp.ServerOptions{
		Instructions: "Read and append CXDB Turn DAG contexts. Read exact turns before treating bounded summaries as complete.",
		Logger:       logger,
	})
	registerTools(server, api)

	stream := mcp.NewStreamableHTTPHandler(func(*http.Request) *mcp.Server { return server }, &mcp.StreamableHTTPOptions{
		Stateless:                    true,
		PropagateRequestCancellation: true,
		MaxRequestBodyBytes:          1 << 20,
		Logger:                       logger,
	})
	originProtected := http.NewCrossOriginProtection().Handler(stream)
	verifier := func(ctx context.Context, token string, req *http.Request) (*mcpauth.TokenInfo, error) {
		for _, candidate := range verifiers {
			var session *cxdbauth.Session
			var verifyErr error
			if requestVerifier, ok := candidate.(cxdbauth.RequestTokenVerifier); ok {
				session, verifyErr = requestVerifier.VerifyWithRequest(req, token)
			} else {
				session, verifyErr = candidate.Verify(token)
			}
			if verifyErr == nil && session != nil {
				return &mcpauth.TokenInfo{
					Scopes: session.Scopes, Expiration: session.ExpiresAt, UserID: session.Issuer + "|" + session.Subject,
				}, nil
			}
		}
		return nil, fmt.Errorf("%w: bearer token is invalid", mcpauth.ErrInvalidToken)
	}
	return mcpauth.RequireBearerToken(verifier, &mcpauth.RequireBearerTokenOptions{
		ResourceMetadataURL: resourceMetadataURL,
		Scopes:              []string{"cxdb:read"},
	})(originProtected), nil
}

type backendClient struct {
	base   *url.URL
	client *http.Client
}

type listInput struct {
	Limit int `json:"limit,omitempty"`
}

type searchInput struct {
	Query string `json:"query"`
	Limit int    `json:"limit,omitempty"`
}

type contextInput struct {
	ContextID string `json:"context_id"`
}

type turnsInput struct {
	ContextID   string `json:"context_id"`
	Limit       int    `json:"limit,omitempty"`
	BeforeTurn  string `json:"before_turn_id,omitempty"`
	ExactTurnID string `json:"turn_id,omitempty"`
}

type createInput struct {
	BaseTurnID string `json:"base_turn_id,omitempty"`
}

type appendMessageInput struct {
	ContextID string `json:"context_id"`
	Role      string `json:"role"`
	Text      string `json:"text"`
}

type appendRawInput struct {
	ContextID     string `json:"context_id"`
	TypeID        string `json:"type_id"`
	TypeVersion   uint32 `json:"type_version"`
	PayloadBase64 string `json:"payload_base64"`
}

func registerTools(server *mcp.Server, api *backendClient) {
	mcp.AddTool(server, &mcp.Tool{Name: "cxdb_list_contexts", Description: "List recent CXDB contexts."}, func(ctx context.Context, _ *mcp.CallToolRequest, input listInput) (*mcp.CallToolResult, map[string]any, error) {
		if err := requireScope(ctx, "cxdb:read"); err != nil {
			return nil, nil, err
		}
		limit := boundedLimit(input.Limit)
		return api.call(ctx, http.MethodGet, "/v1/contexts?limit="+strconv.Itoa(limit), nil)
	})
	mcp.AddTool(server, &mcp.Tool{Name: "cxdb_search_contexts", Description: "Search contexts with CXDB Query Language."}, func(ctx context.Context, _ *mcp.CallToolRequest, input searchInput) (*mcp.CallToolResult, map[string]any, error) {
		if err := requireScope(ctx, "cxdb:read"); err != nil {
			return nil, nil, err
		}
		if strings.TrimSpace(input.Query) == "" {
			return nil, nil, errors.New("query is required")
		}
		path := "/v1/contexts/search?q=" + url.QueryEscape(input.Query) + "&limit=" + strconv.Itoa(boundedLimit(input.Limit))
		return api.call(ctx, http.MethodGet, path, nil)
	})
	mcp.AddTool(server, &mcp.Tool{Name: "cxdb_get_context", Description: "Get one context head and metadata."}, func(ctx context.Context, _ *mcp.CallToolRequest, input contextInput) (*mcp.CallToolResult, map[string]any, error) {
		if err := requireScope(ctx, "cxdb:read"); err != nil {
			return nil, nil, err
		}
		if err := numericID(input.ContextID); err != nil {
			return nil, nil, err
		}
		return api.call(ctx, http.MethodGet, "/v1/contexts/"+input.ContextID, nil)
	})
	mcp.AddTool(server, &mcp.Tool{Name: "cxdb_get_turns", Description: "Read typed turns. Set turn_id to hydrate one exact complete turn."}, func(ctx context.Context, _ *mcp.CallToolRequest, input turnsInput) (*mcp.CallToolResult, map[string]any, error) {
		if err := requireScope(ctx, "cxdb:read"); err != nil {
			return nil, nil, err
		}
		if err := numericID(input.ContextID); err != nil {
			return nil, nil, err
		}
		query := url.Values{"limit": {strconv.Itoa(boundedLimit(input.Limit))}, "view": {"typed"}}
		if input.BeforeTurn != "" {
			if err := numericID(input.BeforeTurn); err != nil {
				return nil, nil, err
			}
			query.Set("before_turn_id", input.BeforeTurn)
		}
		if input.ExactTurnID != "" {
			if err := numericID(input.ExactTurnID); err != nil {
				return nil, nil, err
			}
			query.Set("turn_id", input.ExactTurnID)
		}
		return api.call(ctx, http.MethodGet, "/v1/contexts/"+input.ContextID+"/turns?"+query.Encode(), nil)
	})
	mcp.AddTool(server, &mcp.Tool{Name: "cxdb_get_provenance", Description: "Get provenance for one context."}, func(ctx context.Context, _ *mcp.CallToolRequest, input contextInput) (*mcp.CallToolResult, map[string]any, error) {
		if err := requireScope(ctx, "cxdb:read"); err != nil {
			return nil, nil, err
		}
		if err := numericID(input.ContextID); err != nil {
			return nil, nil, err
		}
		return api.call(ctx, http.MethodGet, "/v1/contexts/"+input.ContextID+"/provenance", nil)
	})
	mcp.AddTool(server, &mcp.Tool{Name: "cxdb_create_context", Description: "Create a new context or fork from a turn."}, func(ctx context.Context, _ *mcp.CallToolRequest, input createInput) (*mcp.CallToolResult, map[string]any, error) {
		if err := requireScope(ctx, "cxdb:write"); err != nil {
			return nil, nil, err
		}
		body := map[string]any{}
		if input.BaseTurnID != "" {
			if err := numericID(input.BaseTurnID); err != nil {
				return nil, nil, err
			}
			body["base_turn_id"] = input.BaseTurnID
		}
		return api.call(ctx, http.MethodPost, "/v1/contexts/create", body)
	})
	mcp.AddTool(server, &mcp.Tool{Name: "cxdb_append_message", Description: "Append a canonical user, assistant, or system message."}, func(ctx context.Context, _ *mcp.CallToolRequest, input appendMessageInput) (*mcp.CallToolResult, map[string]any, error) {
		if err := requireScope(ctx, "cxdb:write"); err != nil {
			return nil, nil, err
		}
		if err := numericID(input.ContextID); err != nil {
			return nil, nil, err
		}
		payload, err := canonicalMessage(input.Role, input.Text)
		if err != nil {
			return nil, nil, err
		}
		body := map[string]any{"type_id": "cxdb.ConversationItem", "type_version": 3, "payload_base64": base64.StdEncoding.EncodeToString(payload)}
		return api.call(ctx, http.MethodPost, "/v1/contexts/"+input.ContextID+"/append", body)
	})
	mcp.AddTool(server, &mcp.Tool{Name: "cxdb_append_turn", Description: "Append a raw MessagePack turn with an explicit registered type."}, func(ctx context.Context, _ *mcp.CallToolRequest, input appendRawInput) (*mcp.CallToolResult, map[string]any, error) {
		if err := requireScope(ctx, "cxdb:write"); err != nil {
			return nil, nil, err
		}
		if err := numericID(input.ContextID); err != nil {
			return nil, nil, err
		}
		if input.TypeID == "" || input.TypeVersion == 0 {
			return nil, nil, errors.New("type_id and type_version are required")
		}
		decoded, err := base64.StdEncoding.DecodeString(input.PayloadBase64)
		if err != nil || len(decoded) > 4<<20 {
			return nil, nil, errors.New("payload_base64 must contain at most 4 MiB")
		}
		body := map[string]any{"type_id": input.TypeID, "type_version": input.TypeVersion, "payload_base64": input.PayloadBase64}
		return api.call(ctx, http.MethodPost, "/v1/contexts/"+input.ContextID+"/append", body)
	})
}

func (c *backendClient) call(ctx context.Context, method, path string, body any) (*mcp.CallToolResult, map[string]any, error) {
	requestURL := *c.base
	parsed, err := url.Parse(path)
	if err != nil {
		return nil, nil, err
	}
	requestURL.Path = parsed.Path
	requestURL.RawQuery = parsed.RawQuery
	var reader io.Reader
	if body != nil {
		raw, marshalErr := json.Marshal(body)
		if marshalErr != nil {
			return nil, nil, marshalErr
		}
		reader = bytes.NewReader(raw)
	}
	req, err := http.NewRequestWithContext(ctx, method, requestURL.String(), reader)
	if err != nil {
		return nil, nil, err
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.client.Do(req)
	if err != nil {
		return nil, nil, fmt.Errorf("CXDB backend request: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, maxBackendResponse+1))
	if err != nil {
		return nil, nil, err
	}
	if len(raw) > maxBackendResponse {
		return nil, nil, errors.New("CXDB backend response exceeds 8 MiB")
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, nil, fmt.Errorf("CXDB backend returned %d: %s", resp.StatusCode, strings.TrimSpace(string(raw)))
	}
	var decoded any
	if err := json.Unmarshal(raw, &decoded); err != nil {
		return nil, nil, fmt.Errorf("decode CXDB response: %w", err)
	}
	return nil, map[string]any{"response": decoded}, nil
}

func requireScope(ctx context.Context, scope string) error {
	info := mcpauth.TokenInfoFromContext(ctx)
	if info == nil {
		return errors.New("authentication is required")
	}
	for _, granted := range info.Scopes {
		if granted == scope {
			return nil
		}
	}
	return fmt.Errorf("insufficient scope: %s is required", scope)
}

func numericID(value string) error {
	if value == "" {
		return errors.New("ID is required")
	}
	if _, err := strconv.ParseUint(value, 10, 64); err != nil {
		return errors.New("ID must be an unsigned integer")
	}
	return nil
}

func boundedLimit(value int) int {
	if value <= 0 {
		return 50
	}
	if value > 200 {
		return 200
	}
	return value
}

func canonicalMessage(role, text string) ([]byte, error) {
	if text == "" {
		return nil, errors.New("text is required")
	}
	item := map[string]any{"status": "complete", "timestamp": time.Now().UnixMilli()}
	switch role {
	case "user":
		item["item_type"] = "user_input"
		item["user_input"] = map[string]any{"text": text}
	case "assistant":
		item["item_type"] = "assistant_turn"
		item["turn"] = map[string]any{"text": text}
	case "system":
		item["item_type"] = "system"
		item["system"] = map[string]any{"text": text, "kind": "info"}
	default:
		return nil, errors.New("role must be user, assistant, or system")
	}
	return msgpack.Marshal(item)
}
