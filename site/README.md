# Showcase site

The showcase site of Flottille (`docs/remote.md`, "The repository"). It is coming: nothing lives
here yet but the OAuth client metadata document.

A change under this folder, like one under `apps/android/`, does not run the Rust suite in CI, except one under `oauth/`.

`oauth/client.json` is Flottille's client metadata document, the client id it signs in to MCP
servers with (`docs/reference/mcp-authorization.md`, §2). The `pages` workflow publishes this folder
alone on GitHub Pages, at `https://kristenjestin.github.io/flottille/`, on a push to `dev` that
changes it and by hand. Its `client_id` is its own address: moving it makes every server see a new
client. A test of the daemon keeps it in agreement with the code.
