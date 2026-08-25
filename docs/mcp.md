# CXDB remote MCP

CXDB serves Streamable HTTP MCP at `/mcp`. It uses the current 2026-07-28 protocol through the official Go MCP SDK. The endpoint is stateless and validates cross-origin requests.

## Authentication

The gateway publishes OAuth protected-resource metadata at `/.well-known/oauth-protected-resource/mcp` and authorization-server metadata at `/.well-known/oauth-authorization-server`.

Remote clients use OAuth 2.1 authorization code flow with PKCE S256. CXDB is the OAuth authorization server. It delegates the browser identity check to the configured OIDC provider. Dynamic client registration is available at `/oauth/register`. Redirect URIs must use HTTPS or loopback HTTP.

Personal API tokens also work as MCP bearer tokens. Create them in the Web UI. A token needs `cxdb:read` to connect. Write tools also require `cxdb:write`.

## Tools

- `cxdb_list_contexts`
- `cxdb_search_contexts`
- `cxdb_get_context`
- `cxdb_get_turns`
- `cxdb_get_provenance`
- `cxdb_create_context`
- `cxdb_append_message`
- `cxdb_append_turn`

Use `turn_id` with `cxdb_get_turns` to hydrate one complete turn. A bounded list response is a summary and can contain truncated strings.

## Security boundary

The gateway protects `/mcp` and gateway `/v1` routes. The direct Rust binary port 9009 and HTTP port 9010 keep their current behavior. Do not expose those direct ports when the gateway must be the authentication boundary.
