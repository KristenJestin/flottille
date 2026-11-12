# Reference: MCP authorization, and what a connection follows of it

A connection to a remote MCP server (`docs/extensions.md`, "Connections") may have to sign in
with OAuth rather than send a key: the server answers `401` until it is given an access token
that its authorization server issued for it. This file summarises what the MCP specification asks
of a client in that case, and says what Flottille does of each point, what it leaves out and why.
The code follows this file (`crates/flottille/src/extensions/connections/oauth.rs`).

**Revision followed: 2026-07-28**, the current one when this was written (November 2026). Its
pages, to read rather than this summary when in doubt:

- [Authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)
- [Authorization Server Discovery](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/authorization-server-discovery)
- [Client Registration](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/client-registration)
- [Security Considerations](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/security-considerations)

It builds on OAuth 2.1 (draft-ietf-oauth-v2-1-13), RFC 6750 (bearer tokens), RFC 8414
(authorization server metadata), RFC 7591 (dynamic client registration), RFC 8707 (resource
indicators), RFC 9728 (protected resource metadata), RFC 9207 (issuer in the authorization
response) and OpenID Connect Discovery 1.0.

## Who is who

| Role | Here |
|---|---|
| MCP server (resource server) | The connection's server, at its declared URL. |
| MCP client (OAuth client) | Flottille's daemon: a **public client** (no secret of its own), a native application. |
| Authorization server | The one the server names; it signs the user in, in their browser. |
| User agent | The user's default browser, opened by the client they asked from (the window, the command line). |

Authorization is for HTTP transports only: a connection that is a command takes its secrets in its
environment, from the vault, as the specification says a stdio server should.

## 1. Discovery

**The specification.** An unauthorised request is answered `401 Unauthorized` with a
`WWW-Authenticate: Bearer` header, which may carry `resource_metadata` (the URL of the protected
resource metadata, RFC 9728) and `scope`. Without `resource_metadata`, the client tries the
well-known URIs, in this order: `/.well-known/oauth-protected-resource<path of the MCP URL>`,
then `/.well-known/oauth-protected-resource`. The metadata lists `authorization_servers` (one at
least); the client chooses one.

The authorization server's metadata is then found at, for an issuer with a path
(`https://auth.example.com/tenant1`): `/.well-known/oauth-authorization-server/tenant1`,
`/.well-known/openid-configuration/tenant1`, `/tenant1/.well-known/openid-configuration`; for an
issuer without one: `/.well-known/oauth-authorization-server`, `/.well-known/openid-configuration`.
The document's `issuer` **must be identical** to the issuer the URL was built from, or it is not
used.

**Flottille.** All of it, in that order. The first authorization server listed is used. Every
document is fetched without a proxy and without following a redirect, as a connection's own
requests are, and every URL it names (metadata, endpoints) must be `https://`, or `http://` on
this machine (the same rule as a connection's URL): a token never crosses a network in clear. The
protected resource metadata's `resource` must be the server's own origin and, by path segments
(`/mc` is not `/mcp`), a prefix of its URL; it is the `resource` Flottille asks tokens for.

**The endpoints are on the issuer's origin.** The authorization server's metadata is chosen by the
MCP server, which may be hostile: it could name a real provider as the issuer and its own host as
the token endpoint, and the code and its PKCE verifier would go to it. So the `token_endpoint` and
the `registration_endpoint` must be of the same origin (scheme, host and port) as the issuer, or
the sign-in is refused. An `authorization_endpoint` on another origin is allowed only when the
authorization response carries an `iss` equal to the issuer (RFC 9207, below), whatever the
metadata says of it: the page the user signs in on may be a sibling host, but only the issuer
itself vouches for what comes back.

## 2. Client registration

**The specification**, in this order of preference: a client id known in advance
(pre-registration), a Client ID Metadata Document (an `https://` URL the client hosts, which
serves as its id), Dynamic Client Registration (RFC 7591, deprecated in this revision and kept for
the servers that have not moved), and last asking the user. Registered credentials are bound to
the issuer that gave them and are never used with another.

**Flottille.**

- A **client id the user declares** in the connection (`client_id`) is used first: the user
  registered Flottille with that server themselves.
- **No Client ID Metadata Document.** It needs a document at a public `https://` URL that the
  authorization server fetches: Flottille runs on the user's machine and hosts nothing. It is the
  reason the user may declare a client id.
- Otherwise **Dynamic Client Registration** when the metadata has a `registration_endpoint`:
  `client_name` "Flottille", the loopback `redirect_uris`, `grant_types` `authorization_code` and
  `refresh_token`, `response_types` `code`, `token_endpoint_auth_method` `none`,
  `application_type` `native`. A secret it gives back is kept with the tokens and sent to the
  token endpoint (`client_secret_post`).
- Neither: the sign-in is refused, saying that the server needs a client id to be declared.
- A registration is made at each sign-in, and kept with the tokens it is for, with their issuer.

## 3. The sign-in

**The specification.** Authorization code with **PKCE**: the client must use `S256`, and must
refuse to go on when the metadata has no `code_challenge_methods_supported` (it means no PKCE);
`state` should be sent and checked, and a response with a missing or other `state` discarded. The
`resource` parameter (RFC 8707) **must** be sent in the authorization request and in every token
request, as the server's canonical URI. Scopes: those of the `WWW-Authenticate` challenge, else
the metadata's `scopes_supported`, else no `scope` at all. Before the browser is opened the
client records the issuer, and on the response it applies RFC 9207: an `iss` present must equal
the recorded issuer exactly (no normalisation); absent, the response is refused only when the
metadata says `authorization_response_iss_parameter_supported: true`. An error response that
fails that check is not acted on nor shown.

**Flottille.**

- PKCE `S256` always: 32 random bytes as the verifier, its SHA-256 as the challenge, both
  base64url. Metadata without `S256` among `code_challenge_methods_supported` is refused.
- `state`: 16 random bytes, base64url, checked on the redirect.
- The redirect is a **loopback** one (OAuth for native apps, RFC 8252): `http://127.0.0.1:<port>/callback`,
  on a port the system picks, listened to by the daemon for **five minutes** and for this one
  sign-in alone. A request with another `state`, or without one, is answered as refused and left
  aside, its code never exchanged: a stray page cannot end the user's sign-in nor slip a code in.
  The first redirect with the right `state` ends it, the listener is closed.
- `iss` checked as the paragraph above says, and required too when the authorization endpoint is
  not on the issuer's origin.
- The browser page that receives the redirect says only whether the sign-in was done, never a
  code or a token.
- The authorization URL is opened by the client the user asked from, in their default browser
  (the system's handler, as every link Flottille opens); the daemon opens nothing.

## 4. Tokens

**The specification.** `Authorization: Bearer <token>` on **every** request, never in the URL; a
token is sent only to the server it was issued for. Refresh tokens are kept confidential; the
client may not assume it gets one. An invalid or expired token is answered `401`.

**Flottille.**

- The tokens (access, refresh, their expiry, the client id and secret, the issuer, the token
  endpoint and the resource) are **one vault entry bound to the connection**
  (`docs/vault.md`, "Bound to one use"): `<name>.oauth`, `for = "connection:<name>"`, every
  project's. In the system's keyring, never in a file nor the store; no method, event, log or
  error says them, and each token is masked in everything the daemon stores and shows from the
  moment it is received (and again from the keyring at each start of the daemon).
- **Refreshed before they expire**: when the server is started and the access token expires
  within 60 seconds, it is refreshed first (`grant_type=refresh_token`, with `resource`); a server
  that runs when its token comes within those 60 seconds is started again, before the call, with
  the refreshed one. A refresh token given back replaces the old one.
- **The tokens are written under the connection's turn**, the lock a call, a review and a start of
  its server hold: a sign-in that ends while a refresh runs waits for it, and writes its own
  tokens after; and a refresh whose refresh token is no longer the stored one (a sign-in replaced
  it) discards its result, whether it was given or refused.
- **A refresh whose tokens cannot be written** (the keyring is locked or busy) is written again
  once; if the vault still refuses, the new tokens are kept in the daemon's memory for as long as
  it runs, and used, never the old refresh token, which the authorization server spent. The inbox
  says so (`connection.tokens_unsaved`); the next refresh writes them again, and stopping the
  daemon first loses them (the connection is then signed out at its next refresh).
- **A refused refresh signs the connection out**: the token endpoint answers `invalid_grant` (the
  refresh token was revoked or has expired), or `401 invalid_client`. Its tokens are removed from
  the vault, its tool leaves every session, and the inbox asks the user to sign in again. Any other
  answer (`temporarily_unavailable`, `invalid_request`, a `5xx`) and an authorization server that
  cannot be reached keep the tokens: the call fails saying to try again, as a server that fails to
  start does.
- No token at all, or a server that still answers `401`: the call fails saying that the user must
  sign in (`flottille connection login <name>`).

## What is left out

- **Client ID Metadata Documents** (above: Flottille hosts nothing).
- **Step-up authorization** (a `403 insufficient_scope` at run time): the call fails with the
  server's words; signing in again asks for the scopes the server challenges with then.
- **More than one authorization server**: the first listed is used.
- **`offline_access`**: not added. The specification lets a client add it; a server that gives no
  refresh token makes the user sign in again once the access token expires.
