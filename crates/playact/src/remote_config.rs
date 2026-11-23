//! Remote configuration: fetch a `TreeSpec` over HTTP(S), refresh it on an
//! interval, and preserve blackboard data.
//!
//! This is the optional `remote-config` feature, which uses `reqwest::blocking`
//! and does not pull in an async runtime. Refreshes run synchronously on the
//! thread calling `poll`/`tick`, so in high-latency environments move refreshing
//! to a worker thread or use a custom `Client` with a timeout.
//!
//! Refresh semantics match hot reload:
//! - Once a new configuration builds successfully it replaces the old tree (root
//!   and configured services), blackboard data is preserved, and node-internal
//!   state is reset by default;
//! - With [`RemoteTreeLoader::with_state_preservation`] enabled, node state is
//!   also restored by stable `NodeId` for nodes that still exist; renamed or
//!   removed IDs start from their reset state;
//! - Each successful refresh records a [`crate::TreeDiff`] with nodes added,
//!   removed, or moved according to their `NodeId` and structural path;
//! - On parse/build/network failure the old tree stays untouched and the error
//!   is returned;
//! - ETag / Last-Modified conditional requests are supported, and a 304 skips
//!   rebuilding;
//! - Tracers are not preserved; reinstall them through
//!   [`RemoteTreeLoader::tree_mut`] after a refresh.

use std::io::Read;
use std::time::{Duration, Instant};

pub use reqwest;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, IF_MODIFIED_SINCE, IF_NONE_MATCH};

use crate::config::{
    ConfigBuildError, ConfigError, IncludeResolver, NodeRegistry, TreeParser, TreeSpec,
    ValidationLimits,
};
use crate::diff::TreeDiff;
use crate::status::Status;
use crate::time::Time;
use crate::tree::BehaviorTree;

/// Maximum response body accepted from a remote configuration endpoint.
pub const MAX_REMOTE_CONFIG_BYTES: usize = 1024 * 1024;

/// Verifies remote configuration responses before parsing.
///
/// Implement this for signatures, MACs, allowlists, or other transport-level
/// checks. The verifier runs on the initial load and every refresh, before any
/// configuration is deserialized.
pub trait ResponseVerifier: Send + Sync {
    /// Returns an error message when the response must be rejected.
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<(), String>;
}

/// Remote configuration error.
#[derive(Debug)]
#[non_exhaustive]
pub enum RemoteConfigError {
    /// HTTP request or response error.
    Http(reqwest::Error),
    /// Failed to read the response body.
    Io(std::io::Error),
    /// The response body exceeded [`MAX_REMOTE_CONFIG_BYTES`].
    ResponseTooLarge {
        /// Maximum accepted body size.
        limit: usize,
    },
    /// A [`ResponseVerifier`] rejected the response.
    Verification(String),
    /// Failed to parse or validate the configuration text.
    Parse(ConfigError),
    /// Configuration parsed but building the behavior tree failed.
    Build(ConfigBuildError),
    /// The server returned an unexpected 304 Not Modified on the initial load.
    NotModified,
}

impl std::fmt::Display for RemoteConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(error) => write!(f, "remote-config HTTP error: {error}"),
            Self::Io(error) => write!(f, "remote-config body read error: {error}"),
            Self::ResponseTooLarge { limit } => {
                write!(f, "remote-config response exceeds {limit} bytes")
            }
            Self::Verification(message) => {
                write!(f, "remote-config response verification failed: {message}")
            }
            Self::Parse(error) => write!(f, "remote-config parse error: {error}"),
            Self::Build(error) => write!(f, "remote-config build error: {error}"),
            Self::NotModified => f.write_str("remote-config initial response was 304 Not Modified"),
        }
    }
}

impl std::error::Error for RemoteConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Build(error) => Some(error),
            Self::ResponseTooLarge { .. } | Self::Verification(_) | Self::NotModified => None,
        }
    }
}

impl From<reqwest::Error> for RemoteConfigError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<ConfigError> for RemoteConfigError {
    fn from(value: ConfigError) -> Self {
        Self::Parse(value)
    }
}

impl From<ConfigBuildError> for RemoteConfigError {
    fn from(value: ConfigBuildError) -> Self {
        Self::Build(value)
    }
}

type Fetched<W> = Option<(BehaviorTree<W>, Option<String>, Option<String>)>;

/// Remote configuration loader.
pub struct RemoteTreeLoader<W: 'static> {
    url: String,
    client: Client,
    registry: NodeRegistry<W>,
    parser: TreeParser,
    resolver: Option<Box<dyn IncludeResolver>>,
    verifier: Option<Box<dyn ResponseVerifier>>,
    tree: BehaviorTree<W>,
    refresh_interval: Duration,
    last_refresh: Instant,
    etag: Option<String>,
    last_modified: Option<String>,
    preserve_state: bool,
    last_diff: Option<TreeDiff>,
}

impl<W: 'static> RemoteTreeLoader<W> {
    /// Creates a loader with a custom parser and the default `Client`; a failed
    /// initial load returns an error.
    pub fn with_parser(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser_impl(url, registry, parser, None, None)
    }

    /// Creates a loader with a custom parser and include resolver.
    ///
    /// The resolver runs again on the initial load and on every refresh; a 304
    /// response does not rebuild the tree, so changes to include contents require
    /// a changed ETag/Last-Modified or a manual reload.
    pub fn with_parser_and_resolver(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        resolver: impl IncludeResolver + 'static,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser_impl(url, registry, parser, Some(Box::new(resolver)), None)
    }

    /// Creates a loader with a response verifier.
    ///
    /// The verifier runs on the initial fetch and every refresh, before parsing.
    pub fn with_parser_and_verifier(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        verifier: impl ResponseVerifier + 'static,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser_impl(url, registry, parser, None, Some(Box::new(verifier)))
    }

    fn with_parser_impl(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        resolver: Option<Box<dyn IncludeResolver>>,
        verifier: Option<Box<dyn ResponseVerifier>>,
    ) -> Result<Self, RemoteConfigError> {
        let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
        Self::with_client_impl(client, url, registry, parser, resolver, verifier)
    }

    /// Creates a loader with a custom `Client`, which is convenient for
    /// configuring proxies, timeouts, and TLS.
    pub fn with_client(
        client: Client,
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_client_impl(client, url, registry, parser, None, None)
    }

    /// Creates a loader with a custom `Client` and include resolver.
    pub fn with_client_and_resolver(
        client: Client,
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        resolver: impl IncludeResolver + 'static,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_client_impl(
            client,
            url,
            registry,
            parser,
            Some(Box::new(resolver)),
            None,
        )
    }

    /// Creates a loader with a custom `Client` and response verifier.
    pub fn with_client_and_verifier(
        client: Client,
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        verifier: impl ResponseVerifier + 'static,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_client_impl(
            client,
            url,
            registry,
            parser,
            None,
            Some(Box::new(verifier)),
        )
    }

    fn with_client_impl(
        client: Client,
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        resolver: Option<Box<dyn IncludeResolver>>,
        verifier: Option<Box<dyn ResponseVerifier>>,
    ) -> Result<Self, RemoteConfigError> {
        let url = url.into();
        let Some((tree, etag, last_modified)) = fetch(FetchRequest {
            client: &client,
            url: &url,
            parser,
            registry: &registry,
            resolver: resolver.as_deref(),
            verifier: verifier.as_deref(),
            etag: None,
            last_modified: None,
        })?
        else {
            return Err(RemoteConfigError::NotModified);
        };

        Ok(Self {
            url,
            client,
            registry,
            parser,
            resolver,
            verifier,
            tree,
            refresh_interval: Duration::from_secs(30),
            last_refresh: Instant::now(),
            etag,
            last_modified,
            preserve_state: false,
            last_diff: None,
        })
    }

    /// Creates a loader for a remote JSON configuration.
    pub fn json(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser(url, registry, TreeSpec::from_json_str)
    }

    /// Creates a JSON loader with a response verifier.
    pub fn json_with_verifier(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        verifier: impl ResponseVerifier + 'static,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser_and_verifier(url, registry, TreeSpec::from_json_str, verifier)
    }

    /// Creates a loader for a remote JSON configuration and an include resolver.
    pub fn json_with_resolver(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        resolver: impl IncludeResolver + 'static,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser_and_resolver(url, registry, TreeSpec::from_json_str, resolver)
    }

    /// Creates a loader for a remote TOML configuration.
    #[cfg(feature = "toml")]
    pub fn toml(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser(url, registry, TreeSpec::from_toml_str)
    }

    /// Creates a loader for a remote YAML configuration.
    #[cfg(feature = "yaml")]
    pub fn yaml(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser(url, registry, TreeSpec::from_yaml_str)
    }

    /// Creates a loader for a remote RON configuration.
    #[cfg(feature = "ron")]
    pub fn ron(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser(url, registry, TreeSpec::from_ron_str)
    }

    /// Sets the refresh interval; defaults to 30 seconds.
    #[must_use]
    pub fn with_refresh_interval(mut self, interval: Duration) -> Self {
        self.refresh_interval = interval;
        self
    }

    /// Enables or disables node-state preservation across refreshes.
    ///
    /// When enabled, the loader captures an ID-keyed snapshot before rebuilding
    /// and restores state for nodes whose `NodeId` still exists in the new
    /// configuration. Nodes without an ID, nodes whose ID was renamed or
    /// removed, and nodes that do not support own-state snapshots start from
    /// their reset state. The blackboard is preserved regardless of this option.
    #[must_use]
    pub fn with_state_preservation(mut self, preserve: bool) -> Self {
        self.preserve_state = preserve;
        self
    }

    /// Remote configuration URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Structural changes from the most recent successful refresh.
    #[must_use]
    pub fn last_diff(&self) -> Option<&TreeDiff> {
        self.last_diff.as_ref()
    }

    /// Takes the structural changes from the most recent successful refresh.
    pub fn take_last_diff(&mut self) -> Option<TreeDiff> {
        self.last_diff.take()
    }

    /// The current behavior tree.
    #[must_use]
    pub fn tree(&self) -> &BehaviorTree<W> {
        &self.tree
    }

    /// Mutable behavior tree. Use it to reinstall Tracers after a refresh.
    pub fn tree_mut(&mut self) -> &mut BehaviorTree<W> {
        &mut self.tree
    }

    /// The registry used for building and refreshing.
    #[must_use]
    pub fn registry(&self) -> &NodeRegistry<W> {
        &self.registry
    }

    /// Mutable registry; newly registered nodes take effect on the next refresh.
    pub fn registry_mut(&mut self) -> &mut NodeRegistry<W> {
        &mut self.registry
    }

    /// Fetches according to the refresh interval; returns `true` when a reload
    /// happened, `false` on 304 or when the interval has not elapsed.
    pub fn poll(&mut self) -> Result<bool, RemoteConfigError> {
        if self.last_refresh.elapsed() < self.refresh_interval {
            return Ok(false);
        }
        self.refresh_now()
    }

    /// Fetches once immediately, ignoring the refresh interval.
    ///
    /// Returns `false` on 304, and `true` when a new tree built successfully and
    /// replaced the old one.
    pub fn refresh_now(&mut self) -> Result<bool, RemoteConfigError> {
        self.last_refresh = Instant::now();

        let etag = self.etag.clone();
        let last_modified = self.last_modified.clone();
        let Some((mut new_tree, etag, last_modified)) = fetch(FetchRequest {
            client: &self.client,
            url: &self.url,
            parser: self.parser,
            registry: &self.registry,
            resolver: self.resolver.as_deref(),
            verifier: self.verifier.as_deref(),
            etag: etag.as_deref(),
            last_modified: last_modified.as_deref(),
        })?
        else {
            return Ok(false);
        };

        let snapshot = if self.preserve_state {
            self.tree.snapshot_by_id().ok()
        } else {
            None
        };

        let board = std::mem::take(self.tree.blackboard_mut());
        *new_tree.blackboard_mut() = board;

        if let Some(snapshot) = &snapshot
            && new_tree.restore_by_id_matching(snapshot).is_err()
        {
            new_tree.reset();
        }

        let diff = TreeDiff::between(&self.tree, &new_tree);
        self.tree = new_tree;
        self.etag = etag;
        self.last_modified = last_modified;
        self.last_diff = Some(diff);

        Ok(true)
    }

    /// Refreshes according to the interval first, then ticks the current tree.
    ///
    /// On refresh failure it returns an error and does not tick; to keep using
    /// the old tree, use [`RemoteTreeLoader::tree_mut`] directly.
    pub fn tick(&mut self, world: &mut W, time: Time) -> Result<Status, RemoteConfigError> {
        self.poll()?;
        Ok(self.tree.tick(world, time))
    }
}

struct FetchRequest<'a, W: 'static> {
    client: &'a Client,
    url: &'a str,
    parser: TreeParser,
    registry: &'a NodeRegistry<W>,
    resolver: Option<&'a dyn IncludeResolver>,
    verifier: Option<&'a dyn ResponseVerifier>,
    etag: Option<&'a str>,
    last_modified: Option<&'a str>,
}

fn fetch<W: 'static>(request: FetchRequest<'_, W>) -> Result<Fetched<W>, RemoteConfigError> {
    let FetchRequest {
        client,
        url,
        parser,
        registry,
        resolver,
        verifier,
        etag,
        last_modified,
    } = request;

    let mut request = client.get(url);
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = last_modified {
        request = request.header(IF_MODIFIED_SINCE, last_modified);
    }

    let response = request.send()?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(None);
    }

    let response = response.error_for_status()?;
    let etag = header_string(&response, "etag");
    let last_modified = header_string(&response, "last-modified");
    let headers = response.headers().clone();

    let mut body = Vec::new();
    response
        .take((MAX_REMOTE_CONFIG_BYTES as u64) + 1)
        .read_to_end(&mut body)
        .map_err(RemoteConfigError::Io)?;
    if body.len() > MAX_REMOTE_CONFIG_BYTES {
        return Err(RemoteConfigError::ResponseTooLarge {
            limit: MAX_REMOTE_CONFIG_BYTES,
        });
    }

    if let Some(verifier) = verifier {
        verifier
            .verify(&headers, &body)
            .map_err(RemoteConfigError::Verification)?;
    }

    let text = std::str::from_utf8(&body).map_err(|error| {
        RemoteConfigError::Parse(ConfigError::new(format!(
            "remote config is not valid UTF-8: {error}"
        )))
    })?;
    let spec = parser(text)?;
    spec.validate(&ValidationLimits::default())
        .map_err(RemoteConfigError::Parse)?;
    let tree = match resolver {
        Some(resolver) => spec.build_with_resolver(registry, resolver)?,
        None => spec.build(registry)?,
    };

    Ok(Some((tree, etag, last_modified)))
}

fn header_string(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use crate::blackboard::Key;
    use crate::command::CommandBuffer;
    use crate::context::Context;
    use crate::status::Status;
    use crate::time::Time;

    use super::*;

    static COMMANDS: Key<CommandBuffer<&'static str>> = crate::key!("commands");
    static MARKER: Key<u32> = crate::key!("marker");

    fn registry() -> NodeRegistry<()> {
        let mut registry = NodeRegistry::<()>::new();

        registry
            .action::<()>("old", |_| {
                move |ctx: &mut Context<'_, ()>| {
                    push(ctx, "old");
                    Status::Success
                }
            })
            .expect("register old");
        registry
            .action::<()>("new", |_| {
                move |ctx: &mut Context<'_, ()>| {
                    push(ctx, "new");
                    Status::Success
                }
            })
            .expect("register new");

        registry
    }

    fn push(ctx: &mut Context<'_, ()>, value: &'static str) {
        ctx.blackboard()
            .get_or_insert_with(&COMMANDS, CommandBuffer::new)
            .push(value);
    }

    fn drain(loader: &mut RemoteTreeLoader<()>) -> Vec<&'static str> {
        loader
            .tree_mut()
            .blackboard_mut()
            .get_mut(&COMMANDS)
            .map(|buffer| buffer.drain().collect())
            .unwrap_or_default()
    }

    fn body(action: &str) -> String {
        format!(r#"{{"root":{{"type":"action","name":"{action}"}}}}"#)
    }

    fn limiter_body(action: &str) -> String {
        format!(
            r#"{{"root":{{"type":"identified","id":"limit","child":{{"type":"limiter","max":2,"child":{{"type":"action","name":"{action}"}}}}}}}}"#
        )
    }

    fn service_body(action: &str) -> String {
        format!(
            r#"{{"root":{{"type":"action","name":"{action}"}},"services":[{{"type":"action","name":"{action}"}}]}}"#
        )
    }

    fn serve_sequence(bodies: Vec<String>) -> String {
        serve_sequence_with_headers(bodies, "")
    }

    fn serve_sequence_with_headers(bodies: Vec<String>, extra_headers: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
        let address = listener.local_addr().expect("local address");

        thread::spawn(move || {
            for body in bodies {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        format!("http://{address}/tree.json")
    }

    #[test]
    fn loads_refreshes_and_preserves_blackboard() {
        let url = serve_sequence(vec![body("old"), body("new")]);
        let mut loader = RemoteTreeLoader::<()>::json(url, registry()).expect("remote loader");
        loader.tree_mut().blackboard_mut().set(&MARKER, 42);

        let mut world = ();
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut loader), vec!["old"]);

        assert!(loader.refresh_now().expect("refresh"));
        assert_eq!(loader.tree().blackboard().get(&MARKER), Some(&42));
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut loader), vec!["new"]);
    }

    #[test]
    fn refresh_can_preserve_node_state_by_id() {
        let url = serve_sequence(vec![limiter_body("old"), limiter_body("new")]);
        let mut loader = RemoteTreeLoader::<()>::json(url, registry())
            .expect("remote loader")
            .with_state_preservation(true);

        let mut world = ();
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut loader), vec!["old"]);

        assert!(loader.refresh_now().expect("refresh"));
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut loader), vec!["new"]);
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Failure
        );
    }

    #[test]
    fn refresh_reports_structural_diff() {
        let old = String::from(
            r#"{"root":{"type":"sequence","children":[
                {"type":"identified","id":"a","child":{"type":"action","name":"old"}},
                {"type":"identified","id":"b","child":{"type":"action","name":"old"}}
            ]}}"#,
        );
        let new = String::from(
            r#"{"root":{"type":"sequence","children":[
                {"type":"identified","id":"b","child":{"type":"action","name":"new"}},
                {"type":"identified","id":"c","child":{"type":"action","name":"new"}}
            ]},"services":[
                {"type":"identified","id":"svc","child":{"type":"action","name":"new"}}
            ]}"#,
        );
        let url = serve_sequence(vec![old, new]);
        let mut loader = RemoteTreeLoader::<()>::json(url, registry()).expect("remote loader");
        assert!(loader.last_diff().is_none());

        assert!(loader.refresh_now().expect("refresh"));
        let diff = loader.last_diff().expect("diff");
        assert_eq!(diff.added.len(), 2);
        assert_eq!(diff.added[0].id.as_str(), "c");
        assert_eq!(diff.added[1].id.as_str(), "svc");
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0].id.as_str(), "a");
        assert_eq!(diff.moved.len(), 1);
        assert_eq!(diff.moved[0].id.as_str(), "b");
        assert_eq!(diff.moved[0].from.to_string(), "root.children[1]");
        assert_eq!(diff.moved[0].to.to_string(), "root.children[0]");
        assert!(diff.renamed.is_empty());

        assert!(loader.take_last_diff().is_some());
        assert!(loader.last_diff().is_none());
    }

    #[test]
    fn refresh_rebuilds_configured_services() {
        let url = serve_sequence(vec![service_body("old"), service_body("new")]);
        let mut loader = RemoteTreeLoader::<()>::json(url, registry()).expect("remote loader");

        let mut world = ();
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut loader), vec!["old", "old"]);

        assert!(loader.refresh_now().expect("refresh"));
        assert_eq!(loader.tree().service_count(), 1);
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut loader), vec!["new", "new"]);
    }

    #[test]
    fn loads_config_with_includes() {
        let url = serve_sequence(vec![
            r#"{"root":{"type":"include","path":"sub"}}"#.to_string(),
        ]);
        let resolver = |path: &str| -> Result<TreeSpec, ConfigError> {
            match path {
                "sub" => TreeSpec::from_json_str(&body("old")),
                other => Err(ConfigError::new(format!("unknown include `{other}`"))),
            }
        };

        let mut loader = RemoteTreeLoader::<()>::json_with_resolver(url, registry(), resolver)
            .expect("remote loader");
        let mut world = ();
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut loader), vec!["old"]);
    }

    #[test]
    fn invalid_payload_returns_parse_error() {
        let url = serve_sequence(vec![String::from("{")]);
        let error = RemoteTreeLoader::<()>::json(url, registry())
            .err()
            .expect("parse error");
        assert!(matches!(error, RemoteConfigError::Parse(_)));
    }

    #[test]
    fn oversized_payload_is_rejected_before_parsing() {
        let url = serve_sequence(vec!["x".repeat(MAX_REMOTE_CONFIG_BYTES + 1)]);
        let error = RemoteTreeLoader::<()>::json(url, registry())
            .err()
            .expect("oversized payload");
        assert!(matches!(
            error,
            RemoteConfigError::ResponseTooLarge { limit } if limit == MAX_REMOTE_CONFIG_BYTES
        ));
    }

    struct SignatureVerifier(&'static str);

    impl ResponseVerifier for SignatureVerifier {
        fn verify(&self, headers: &HeaderMap, payload: &[u8]) -> Result<(), String> {
            let signature = headers
                .get("x-signature")
                .and_then(|value| value.to_str().ok());
            if signature == Some(self.0) && payload == body("old").as_bytes() {
                Ok(())
            } else {
                Err(String::from("invalid signature"))
            }
        }
    }

    #[test]
    fn response_verifier_can_accept_or_reject_payloads() {
        let url = serve_sequence_with_headers(vec![body("old")], "X-Signature: good\r\n");
        let mut loader =
            RemoteTreeLoader::<()>::json_with_verifier(url, registry(), SignatureVerifier("good"))
                .expect("verified loader");
        let mut world = ();
        assert_eq!(
            loader.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );

        let url = serve_sequence_with_headers(vec![body("old")], "X-Signature: bad\r\n");
        let error =
            RemoteTreeLoader::<()>::json_with_verifier(url, registry(), SignatureVerifier("good"))
                .err()
                .expect("verification error");
        assert!(matches!(
            error,
            RemoteConfigError::Verification(message) if message == "invalid signature"
        ));
    }
}
