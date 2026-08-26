# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

### Added

- Add authenticated Streamable HTTP MCP with scoped read and write tools.
- Add OAuth 2.1 with PKCE, dynamic client registration, and browser OIDC.
- Add personal API tokens with scopes, expiry, one-time display, and revocation.
- Add OpenAPI JSON and YAML documents, `llms.txt`, and client authentication guidance.
- Add batch append and blob reads, bounded trace pages, exact turn hydration, and faster typed projection.
- Add responsive token management and debugger views for mobile devices.

### Changed

- Enforce write scopes and stricter proxy-header handling.
- Correct separate tool-result hydration and named-key MessagePack projection. Numeric field tags continue to have priority.

### Fixed

- Pin pnpm 9 in Node 20 container builds.

### Compatibility

- Gateway deployments must now provide a `SESSION_SECRET` of at least 32 bytes.
- Non-GET API requests now require authentication with the `cxdb:write` scope.
- Scoped credentials used with context, metrics, and event reads must include `cxdb:read`. Existing browser sessions and built-in service credentials receive both scopes.
- There are no known breaking stored-data changes.
