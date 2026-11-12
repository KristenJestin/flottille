//! Signing a connection in with OAuth, as the MCP specification's authorization asks of a client
//! (`docs/reference/mcp-authorization.md`, revision 2026-07-28): the server's `401` challenge, its
//! protected resource metadata, its authorization server's metadata, a client id (declared, or
//! registered dynamically), the authorization code with PKCE `S256`, `state` and `resource`
//! received on a loopback redirect, and tokens refreshed before they expire. The tokens are one
//! vault entry bound to the connection: never in a file, the store, an event, a log or an error,
//! and each is masked from the moment it is received.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use flottille_core::secrets;
use flottille_core::store::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{Declaration, Reach};
use crate::daemon::Daemon;

/// How long the loopback waits for the browser to come back.
const SIGN_IN_LIMIT: Duration = Duration::from_secs(300);
/// How long each request to the server or its authorization server has.
const REQUEST_LIMIT: Duration = Duration::from_secs(30);
/// A token that expires within this is refreshed before it is used.
pub const MARGIN_MS: u64 = 60_000;
/// The longest answer read from an authorization server or a browser.
const LONGEST: usize = 64 * 1024;

/// What a sign-in keeps, as the secret of the vault entry `<name>.oauth`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Tokens {
    /// The server's URL it was signed in for.
    url: String,
    /// The `resource` the tokens are for (RFC 8707).
    resource: String,
    issuer: String,
    token_endpoint: String,
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    /// When the access token expires, in milliseconds since the epoch; none said, never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

impl Tokens {
    /// Its secrets, each masked from now on: the entry's whole value is not what a leak shows.
    fn masked(&self) {
        for secret in [Some(&self.access_token), self.refresh_token.as_ref()]
            .into_iter()
            .chain([self.client_secret.as_ref()])
            .flatten()
        {
            secrets::add_vault_value(secret);
        }
    }

    fn expiring(&self) -> bool {
        self.expires_at
            .is_some_and(|at| at <= now_ms() as u64 + MARGIN_MS)
    }
}

/// The secrets an entry's value holds when it is a sign-in's tokens: what the daemon masks at its
/// start, beside the whole value.
pub fn secrets_in(value: &str) -> Vec<String> {
    let Ok(tokens) = serde_json::from_str::<Tokens>(value) else {
        return Vec::new();
    };
    [
        Some(tokens.access_token),
        tokens.refresh_token,
        tokens.client_secret,
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The vault entry of a connection's tokens.
pub fn entry(name: &str) -> String {
    format!("{name}.oauth")
}

/// Why a token could not be had for the server.
pub enum Unusable {
    /// The authorization server refused the refresh: the user must sign in again.
    SignedOut(String),
    /// Anything else: the connection fails to start, as a server that cannot be reached.
    Failed(String),
}

/// The tokens kept for the connection, if it signed in.
async fn kept(daemon: &Daemon, name: &str) -> Result<Option<Tokens>, String> {
    let entries = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?;
    if !entries.iter().any(|kept| kept.name == entry(name)) {
        return Ok(None);
    }
    let value = crate::vault::every_project_s(daemon, name, &entry(name)).await?;
    let tokens: Tokens = serde_json::from_str(&value)
        .map_err(|_| format!("the vault entry {} holds no sign-in", entry(name)))?;
    tokens.masked();
    Ok(Some(tokens))
}

/// Keeps the tokens in the vault, bound to the connection, every project's.
async fn keep(daemon: &Arc<Daemon>, name: &str, tokens: &Tokens) -> Result<(), String> {
    tokens.masked();
    crate::vault::call(
        daemon.clone(),
        "vault.set",
        json!({"name": entry(name), "secret": serde_json::to_string(tokens).expect("JSON"),
            "description": format!("The OAuth tokens of the connection {name}, kept by its \
                sign-in (flottille connection login {name})."),
            "url": tokens.url, "for": flottille_core::vault::connection(name)}),
    )
    .await
    .map(|_| ())
    .map_err(|failure| failure.message)
}

/// Removes the connection's tokens, if it has any.
pub async fn forget(daemon: &Arc<Daemon>, name: &str) -> Result<(), String> {
    match crate::vault::call(
        daemon.clone(),
        "vault.remove",
        json!({"name": entry(name), "confirmed": true}),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(failure) if failure.code == "not-found" => Ok(()),
        Err(failure) => Err(failure.message),
    }
}

/// The access token the server is reached with, refreshed first when it expires within a minute,
/// and when it expires; none when the connection never signed in.
pub async fn bearer(
    daemon: &Arc<Daemon>,
    declaration: &Declaration,
) -> Result<Option<(String, Option<u64>)>, Unusable> {
    let Reach::Url(url) = &declaration.reach else {
        return Ok(None);
    };
    let Some(tokens) = kept(daemon, &declaration.name)
        .await
        .map_err(Unusable::Failed)?
    else {
        return Ok(None);
    };
    if tokens.url != *url {
        return Err(Unusable::SignedOut(format!(
            "it was signed in for {}, and its URL is {url} now",
            tokens.url
        )));
    }
    let tokens = match tokens.expiring() {
        false => tokens,
        true => {
            let fresh = refreshed(&tokens).await?;
            keep(daemon, &declaration.name, &fresh)
                .await
                .map_err(Unusable::Failed)?;
            fresh
        }
    };
    Ok(Some((tokens.access_token, tokens.expires_at)))
}

/// A refresh of the tokens: the old refresh token is kept when the answer gives no new one.
async fn refreshed(tokens: &Tokens) -> Result<Tokens, Unusable> {
    let Some(refresh) = &tokens.refresh_token else {
        return Err(Unusable::SignedOut(
            "its access token expired, and the authorization server gave no refresh token".into(),
        ));
    };
    let mut form = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh.clone()),
        ("client_id", tokens.client_id.clone()),
        ("resource", tokens.resource.clone()),
    ];
    if let Some(secret) = &tokens.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let (status, answer) = posted_form(&tokens.token_endpoint, &form)
        .await
        .map_err(|why| Unusable::Failed(format!("the sign-in is kept, try again: {why}")))?;
    if !(200..300).contains(&status) {
        let why = refusal(&answer);
        // Only a refresh token that is no good, or a client that is not known, ends the sign-in:
        // an authorization server that is busy or confused keeps it.
        return Err(match (status, answer["error"].as_str()) {
            (400 | 401, Some("invalid_grant")) | (401, Some("invalid_client")) => {
                Unusable::SignedOut(why)
            }
            _ => Unusable::Failed(format!(
                "the sign-in is kept, try again: the refresh answered {status} {why}"
            )),
        });
    }
    let mut fresh = granted(tokens.clone(), &answer).map_err(Unusable::Failed)?;
    if fresh.refresh_token.is_none() {
        fresh.refresh_token = tokens.refresh_token.clone();
    }
    Ok(fresh)
}

/// The tokens a token endpoint's answer gives, over what `base` kept of the sign-in.
fn granted(mut base: Tokens, answer: &Value) -> Result<Tokens, String> {
    let access = answer["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or("the authorization server gave no access token")?;
    if let Some(kind) = answer["token_type"].as_str()
        && !kind.eq_ignore_ascii_case("bearer")
    {
        return Err(format!(
            "the authorization server gave a {kind} token, not a bearer one"
        ));
    }
    base.access_token = access.to_owned();
    base.refresh_token = answer["refresh_token"].as_str().map(str::to_owned);
    base.expires_at = answer["expires_in"]
        .as_u64()
        .map(|seconds| now_ms() as u64 + seconds * 1000);
    if let Some(scope) = answer["scope"].as_str() {
        base.scope = Some(scope.to_owned());
    }
    base.masked();
    Ok(base)
}

/// An OAuth error answer as a sentence, masked.
fn refusal(answer: &Value) -> String {
    let error = answer["error"].as_str().unwrap_or("no reason given");
    let said = match answer["error_description"].as_str() {
        Some(description) => format!("{error}: {description}"),
        None => error.to_owned(),
    };
    super::cut(&secrets::mask(&said, &Default::default()), 300)
}

// ── The requests ─────────────────────────────────────────────────────────────────────────────

/// A URL the sign-in may reach: what a connection may (`https://`, or `http://` on this machine).
fn reachable(text: &str, what: &str) -> Result<String, String> {
    super::checked_url(text).map_err(|why| format!("{what}: {why}"))
}

async fn sent(
    request: reqwest::RequestBuilder,
) -> Result<(u16, reqwest::header::HeaderMap, Vec<u8>), String> {
    let answer = tokio::time::timeout(REQUEST_LIMIT, async {
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await.map_err(|error| error.to_string())?;
        Ok::<_, String>((status, headers, body.to_vec()))
    })
    .await
    .map_err(|_| format!("no answer within {} seconds", REQUEST_LIMIT.as_secs()))??;
    if answer.2.len() > LONGEST {
        return Err("the answer is too long".into());
    }
    Ok(answer)
}

/// A JSON document by GET: its status and the document, `Null` when it is none.
async fn fetched(url: &str) -> Result<(u16, Value), String> {
    let client = super::client::http_client(url)?;
    let (status, _, body) = sent(client.get(url).header("Accept", "application/json")).await?;
    Ok((status, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

async fn posted_form(url: &str, form: &[(&str, String)]) -> Result<(u16, Value), String> {
    let client = super::client::http_client(url)?;
    let body: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().map(|(key, value)| (*key, value.as_str())))
        .finish();
    let (status, _, body) = sent(
        client
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(body),
    )
    .await
    .map_err(|why| format!("the token endpoint: {why}"))?;
    Ok((status, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

// ── Discovery ────────────────────────────────────────────────────────────────────────────────

/// A parameter of a `WWW-Authenticate: Bearer` challenge.
fn challenge_param(header: &str, key: &str) -> Option<String> {
    let mut rest = header;
    while let Some(at) = rest.find(key) {
        let before = rest[..at].chars().last();
        let after = &rest[at + key.len()..];
        rest = after;
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let Some(value) = after.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        return Some(match value.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next().unwrap_or_default().to_owned(),
            None => value
                .split([',', ' '])
                .next()
                .unwrap_or_default()
                .to_owned(),
        });
    }
    None
}

/// The server's challenge to a request without a token: its `resource_metadata` and `scope`.
async fn challenge(url: &str) -> Result<(Option<String>, Option<String>), String> {
    let client = super::client::http_client(url)?;
    let initialize = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "flottille", "version": crate::VERSION}}});
    let (status, headers, _) = sent(
        client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(initialize.to_string()),
    )
    .await
    .map_err(|why| format!("the server: {why}"))?;
    if status != 401 {
        return Err(format!(
            "the server answered {status} without a token: it asks for no sign-in"
        ));
    }
    let header = headers
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| {
            value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("bearer")
        })
        .unwrap_or_default()
        .to_owned();
    Ok((
        challenge_param(&header, "resource_metadata"),
        challenge_param(&header, "scope"),
    ))
}

/// Where a well-known document of `base` is for `path` (RFC 8414 and RFC 9728's insertion).
fn well_known(origin: &str, suffix: &str, path: &str) -> String {
    let path = path.trim_end_matches('/');
    format!("{origin}/.well-known/{suffix}{path}")
}

fn origin_of(url: &url::Url) -> String {
    url.origin().ascii_serialization()
}

/// The protected resource metadata: the `resource` tokens are asked for, the authorization
/// server, and the scopes it lists.
async fn resource_metadata(
    url: &str,
    said: Option<String>,
) -> Result<(String, String, Option<String>), String> {
    let server = url::Url::parse(url).map_err(|error| error.to_string())?;
    let origin = origin_of(&server);
    let candidates = match said {
        Some(said) => vec![reachable(&said, "the resource metadata")?],
        None => {
            let mut all = Vec::new();
            if !matches!(server.path(), "" | "/") {
                all.push(well_known(
                    &origin,
                    "oauth-protected-resource",
                    server.path(),
                ));
            }
            all.push(well_known(&origin, "oauth-protected-resource", ""));
            all
        }
    };
    for candidate in candidates {
        let (status, metadata) = fetched(&candidate).await?;
        if status != 200 || !metadata.is_object() {
            continue;
        }
        let resource = metadata["resource"]
            .as_str()
            .ok_or("the resource metadata names no resource")?
            .to_owned();
        let named = url::Url::parse(&resource).map_err(|error| error.to_string())?;
        if origin_of(&named) != origin || !url.starts_with(resource.trim_end_matches('/')) {
            return Err(format!(
                "the resource metadata is for {resource}, which is not {url}"
            ));
        }
        let authorization = metadata["authorization_servers"][0]
            .as_str()
            .ok_or("the resource metadata names no authorization server")?;
        let scopes = metadata["scopes_supported"].as_array().map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        });
        return Ok((
            resource,
            reachable(authorization, "the authorization server")?,
            scopes.filter(|scopes| !scopes.is_empty()),
        ));
    }
    Err("the server publishes no resource metadata (RFC 9728)".into())
}

/// Whether two URLs are of one origin (scheme, host and port).
fn same_origin(one: &str, other: &str) -> bool {
    match (url::Url::parse(one), url::Url::parse(other)) {
        (Ok(one), Ok(other)) => one.origin() == other.origin(),
        _ => false,
    }
}

/// The authorization server's metadata, its `issuer` the one asked for, PKCE `S256` among what
/// it supports.
async fn authorization_metadata(issuer: &str) -> Result<Value, String> {
    let parsed = url::Url::parse(issuer).map_err(|error| error.to_string())?;
    let origin = origin_of(&parsed);
    let path = parsed.path().trim_end_matches('/');
    let candidates = match path {
        "" => vec![
            well_known(&origin, "oauth-authorization-server", ""),
            well_known(&origin, "openid-configuration", ""),
        ],
        path => vec![
            well_known(&origin, "oauth-authorization-server", path),
            well_known(&origin, "openid-configuration", path),
            format!("{origin}{path}/.well-known/openid-configuration"),
        ],
    };
    for candidate in candidates {
        let (status, metadata) = fetched(&candidate).await?;
        if status != 200 || !metadata.is_object() {
            continue;
        }
        if metadata["issuer"].as_str() != Some(issuer) {
            return Err(format!(
                "the authorization server's metadata is not {issuer}'s: its issuer is {}",
                metadata["issuer"]
            ));
        }
        let s256 = metadata["code_challenge_methods_supported"]
            .as_array()
            .is_some_and(|methods| methods.iter().any(|method| method == "S256"));
        if !s256 {
            return Err(
                "the authorization server does not support PKCE with S256: no sign-in is safe \
                 with it"
                    .into(),
            );
        }
        for endpoint in ["authorization_endpoint", "token_endpoint"] {
            let named = metadata[endpoint]
                .as_str()
                .ok_or_else(|| format!("the authorization server has no {endpoint}"))?;
            reachable(named, endpoint)?;
        }
        // The code and its verifier go to the token endpoint, and a client's registration to
        // the registration endpoint: a server's metadata, which an MCP server's own metadata
        // chose, never sends them to another origin than the issuer's.
        for endpoint in ["token_endpoint", "registration_endpoint"] {
            if let Some(named) = metadata[endpoint].as_str() {
                reachable(named, endpoint)?;
                if !same_origin(named, issuer) {
                    return Err(format!(
                        "the authorization server's {endpoint} ({named}) is not on its issuer's \
                         origin ({issuer}): it is refused"
                    ));
                }
            }
        }
        return Ok(metadata);
    }
    Err(format!(
        "the authorization server {issuer} publishes no metadata (RFC 8414, OpenID Connect)"
    ))
}

/// The client id (and secret) the sign-in uses: the one the user declared, or one registered.
async fn client(
    declaration: &Declaration,
    metadata: &Value,
    redirect: &str,
) -> Result<(String, Option<String>), String> {
    if let Some(id) = &declaration.client_id {
        return Ok((id.clone(), None));
    }
    let Some(endpoint) = metadata["registration_endpoint"].as_str() else {
        return Err(format!(
            "the authorization server registers no client by itself: register Flottille with it \
             and declare its client id (`flottille connection edit {} --client-id <id>`)",
            declaration.name
        ));
    };
    let asked = json!({"client_name": "Flottille", "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"],
        "token_endpoint_auth_method": "none", "application_type": "native"});
    let http = super::client::http_client(endpoint)?;
    let (status, _, body) = sent(
        http.post(endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(asked.to_string()),
    )
    .await
    .map_err(|why| format!("the registration: {why}"))?;
    let answer: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        return Err(format!(
            "the authorization server refused to register Flottille: {}",
            refusal(&answer)
        ));
    }
    let id = answer["client_id"]
        .as_str()
        .ok_or("the registration gave no client id")?;
    let secret = answer["client_secret"].as_str().map(str::to_owned);
    if let Some(secret) = &secret {
        secrets::add_vault_value(secret);
    }
    Ok((id.to_owned(), secret))
}

// ── The sign-in ──────────────────────────────────────────────────────────────────────────────

fn random(bytes: usize) -> Result<String, String> {
    use std::io::Read;
    let mut drawn = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut drawn))
        .map_err(|error| format!("the system's random source: {error}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(drawn))
}

fn challenge_of(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier))
}

/// One sign-in, waiting for the browser.
struct Pending {
    name: String,
    state: String,
    verifier: String,
    redirect: String,
    issuer: String,
    /// The authorization response must carry `iss` (RFC 9207): the metadata says it does, or the
    /// authorization endpoint is on another origin than the issuer's, which only an `iss` that
    /// is the issuer's vouches for.
    says_iss: bool,
    base: Tokens,
}

/// Begins a sign-in of the connection: the address to open in the user's browser. The daemon
/// waits for the browser on a loopback redirect for five minutes, and says how it ended with
/// `connection.signed_in` or `connection.sign_in_failed`. A sign-in begun again replaces the one
/// that waits.
pub async fn begin(daemon: &Arc<Daemon>, declaration: &Declaration) -> Result<String, String> {
    let Reach::Url(url) = &declaration.reach else {
        return Err(format!(
            "the connection {} is a command: it signs in with what its env gives it, never in a \
             browser",
            declaration.name
        ));
    };
    let (said, scope) = challenge(url).await?;
    let (resource, issuer, scopes) = resource_metadata(url, said).await?;
    let metadata = authorization_metadata(&issuer).await?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| format!("no loopback port for the redirect: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let (client_id, client_secret) = client(declaration, &metadata, &redirect).await?;
    let scope = scope.or(scopes);
    let verifier = random(32)?;
    let state = random(16)?;
    let mut address = url::Url::parse(
        metadata["authorization_endpoint"]
            .as_str()
            .expect("checked"),
    )
    .map_err(|error| error.to_string())?;
    {
        let mut query = address.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("code_challenge", &challenge_of(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &resource);
        if let Some(scope) = &scope {
            query.append_pair("scope", scope);
        }
    }
    let pending = Pending {
        name: declaration.name.clone(),
        state,
        verifier,
        redirect,
        issuer: issuer.clone(),
        says_iss: metadata["authorization_response_iss_parameter_supported"] == true
            || !same_origin(
                metadata["authorization_endpoint"].as_str().expect("checked"),
                &issuer,
            ),
        base: Tokens {
            url: url.clone(),
            resource,
            issuer,
            token_endpoint: metadata["token_endpoint"]
                .as_str()
                .expect("checked")
                .to_owned(),
            client_id,
            client_secret,
            scope,
            access_token: String::new(),
            refresh_token: None,
            expires_at: None,
        },
    };
    let weak = Arc::downgrade(daemon);
    let task = tokio::spawn(async move {
        let ended = tokio::time::timeout(SIGN_IN_LIMIT, received(&weak, listener, &pending))
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "the browser did not come back within {} minutes",
                    SIGN_IN_LIMIT.as_secs() / 60
                ))
            });
        if let Some(daemon) = weak.upgrade() {
            super::signed_in(&daemon, &pending.name, ended).await;
        }
    });
    let replaced = daemon
        .extensions
        .connections
        .signing
        .lock()
        .unwrap()
        .insert(declaration.name.clone(), task.abort_handle());
    if let Some(replaced) = replaced {
        replaced.abort();
    }
    Ok(address.into())
}

/// The browser's redirects until the one of this sign-in, whose code is exchanged and whose
/// tokens are kept. A request with another `state`, or none, is answered as refused and left
/// aside: it cannot end the user's sign-in, and its code is never exchanged.
async fn received(
    daemon: &std::sync::Weak<Daemon>,
    listener: tokio::net::TcpListener,
    pending: &Pending,
) -> Result<(), String> {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let Some(query) = request_query(&mut stream).await else {
            answer_page(&mut stream, 404, "Nothing here.").await;
            continue;
        };
        let fields: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();
        if fields.get("state") != Some(&pending.state) {
            answer_page(
                &mut stream,
                400,
                "This is not the end of a sign-in Flottille began: nothing was done.",
            )
            .await;
            continue;
        }
        let ended = exchanged(daemon, pending, &fields).await;
        match &ended {
            Ok(()) => {
                answer_page(
                    &mut stream,
                    200,
                    &format!(
                        "Flottille is signed in to the connection {}. You may close this page.",
                        pending.name
                    ),
                )
                .await
            }
            Err(why) => {
                answer_page(
                    &mut stream,
                    400,
                    &format!(
                        "Flottille could not sign in to the connection {}: {why}",
                        pending.name
                    ),
                )
                .await
            }
        }
        return ended;
    }
}

/// What the redirect brought: the issuer checked (RFC 9207), then the code exchanged for tokens,
/// with its verifier and the resource, and the tokens kept.
async fn exchanged(
    daemon: &std::sync::Weak<Daemon>,
    pending: &Pending,
    fields: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    match fields.get("iss") {
        Some(iss) if *iss != pending.issuer => {
            return Err(
                "the answer came from another authorization server than the one asked".into(),
            );
        }
        None if pending.says_iss => {
            return Err("the answer does not say which authorization server gave it".into());
        }
        _ => {}
    }
    if let Some(error) = fields.get("error") {
        let mut answer = json!({"error": error});
        if let Some(description) = fields.get("error_description") {
            answer["error_description"] = json!(description);
        }
        return Err(format!(
            "the authorization server refused: {}",
            refusal(&answer)
        ));
    }
    let code = fields.get("code").ok_or("the answer holds no code")?;
    let base = &pending.base;
    let mut form = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code.clone()),
        ("redirect_uri", pending.redirect.clone()),
        ("client_id", base.client_id.clone()),
        ("code_verifier", pending.verifier.clone()),
        ("resource", base.resource.clone()),
    ];
    if let Some(secret) = &base.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let (status, answer) = posted_form(&base.token_endpoint, &form).await?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "the authorization server did not give the tokens: {}",
            refusal(&answer)
        ));
    }
    let tokens = granted(base.clone(), &answer)?;
    let daemon = daemon.upgrade().ok_or("the daemon stopped")?;
    keep(&daemon, &pending.name, &tokens).await
}

/// The query of a `GET /callback`, read from the browser's request.
async fn request_query(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = tokio::time::timeout(REQUEST_LIMIT, stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if read == 0 || head.len() > LONGEST {
            return None;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    let line = String::from_utf8_lossy(&head);
    let mut words = line.lines().next()?.split_whitespace();
    if words.next()? != "GET" {
        return None;
    }
    let target = words.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    (path == "/callback").then(|| query.to_owned())
}

async fn answer_page(stream: &mut tokio::net::TcpStream, status: u16, text: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Flottille</title><p>{escaped}</p>\n"
    );
    let answer = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(answer.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_gives_its_resource_metadata_and_scope() {
        let header = "Bearer resource_metadata=\"https://notes.example.com/.well-known/\
                      oauth-protected-resource\", scope=\"files:read files:write\"";
        assert_eq!(
            challenge_param(header, "resource_metadata").as_deref(),
            Some("https://notes.example.com/.well-known/oauth-protected-resource")
        );
        assert_eq!(
            challenge_param(header, "scope").as_deref(),
            Some("files:read files:write")
        );
        assert_eq!(challenge_param("Bearer realm=x", "scope"), None);
    }

    #[test]
    fn the_challenge_of_a_verifier_is_its_s256() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn well_known_addresses_insert_the_path() {
        assert_eq!(
            well_known(
                "https://auth.example.com",
                "oauth-authorization-server",
                "/tenant1"
            ),
            "https://auth.example.com/.well-known/oauth-authorization-server/tenant1"
        );
        assert_eq!(
            well_known("https://auth.example.com", "openid-configuration", ""),
            "https://auth.example.com/.well-known/openid-configuration"
        );
    }
}
