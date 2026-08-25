# Client authentication

CXDB supports browser login and scoped bearer tokens through the gateway.

## Browser login

The gateway uses the configured OpenID Connect (OIDC) provider. The provider
must return a verified identity. The gateway creates a browser session and a
session-bound CSRF token for state-changing requests.

## Personal API tokens

Authenticated users can create personal tokens in the Web UI. A token has a
name, an optional expiry, and one or both of these scopes:

- `cxdb:read` for context and turn reads.
- `cxdb:write` for context creation and turn appends.

The token secret is shown only once. Store it in a secret manager. Send it in
the HTTP header below. Do not put a token in a URL or browser storage.

```http
Authorization: Bearer <token>
```

The Web UI uses `X-CSRF-Token` for create and revoke requests. Token metadata
does not include token secrets. A revoked or expired token cannot be used.

## MCP OAuth

Remote MCP clients can use OAuth 2.1 authorization code flow with PKCE. Use
the protected-resource metadata at
`/.well-known/oauth-protected-resource/mcp` and the authorization-server
metadata at `/.well-known/oauth-authorization-server`.

The gateway delegates browser identity checks to the configured OIDC provider.
Dynamic client registration is available at `/oauth/register`. Redirect URIs
must use HTTPS or loopback HTTP. MCP clients can also use a personal bearer
token with `cxdb:read`; write tools also require `cxdb:write`.

See [MCP guidance](mcp.md) and the published [OpenAPI JSON](../frontend/public/openapi.json).
