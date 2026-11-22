//! 远程配置：从 HTTP(S) 拉取 `TreeSpec`，按间隔刷新并保留黑板数据。
//!
//! 这是可选的 `remote-config` feature，使用 `reqwest::blocking`；不引入
//! async runtime。刷新在调用 `poll`/`tick` 的线程同步执行，因此高延迟环境
//! 应把刷新放到工作线程，或使用自定义 `Client` 设置超时。
//!
//! 刷新语义与热重载一致：
//! - 新配置成功构建后替换旧树，黑板数据保留，节点内部状态重置；
//! - 解析/构建/网络失败时旧树保持不变，错误返回；
//! - 支持 ETag / Last-Modified 条件请求，304 时不重建；
//! - Tracer 不保留，可刷新后通过 [`RemoteTreeLoader::tree_mut`] 重新安装。

use std::time::{Duration, Instant};

pub use reqwest;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use reqwest::header::{IF_MODIFIED_SINCE, IF_NONE_MATCH};

use crate::config::{ConfigBuildError, ConfigError, NodeRegistry, TreeParser, TreeSpec};
use crate::status::Status;
use crate::time::Time;
use crate::tree::BehaviorTree;

/// 远程配置错误。
#[derive(Debug)]
#[non_exhaustive]
pub enum RemoteConfigError {
    /// HTTP 请求或响应错误。
    Http(reqwest::Error),
    /// 配置文本解析失败。
    Parse(ConfigError),
    /// 配置通过解析但构建行为树失败。
    Build(ConfigBuildError),
    /// 初次加载时服务器返回了意外的 304 Not Modified。
    NotModified,
}

impl std::fmt::Display for RemoteConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(error) => write!(f, "remote-config HTTP error: {error}"),
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
            Self::Parse(error) => Some(error),
            Self::Build(error) => Some(error),
            Self::NotModified => None,
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

/// 远程配置加载器。
pub struct RemoteTreeLoader<W: 'static> {
    url: String,
    client: Client,
    registry: NodeRegistry<W>,
    parser: TreeParser,
    tree: BehaviorTree<W>,
    refresh_interval: Duration,
    last_refresh: Instant,
    etag: Option<String>,
    last_modified: Option<String>,
}

impl<W: 'static> RemoteTreeLoader<W> {
    /// 用自定义 parser 和默认 Client 构造；初次加载失败会返回错误。
    pub fn with_parser(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
    ) -> Result<Self, RemoteConfigError> {
        let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
        Self::with_client(client, url, registry, parser)
    }

    /// 用自定义 Client 构造；便于配置代理、超时和 TLS。
    pub fn with_client(
        client: Client,
        url: impl Into<String>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
    ) -> Result<Self, RemoteConfigError> {
        let url = url.into();
        let Some((tree, etag, last_modified)) =
            fetch(&client, &url, parser, &registry, None, None)?
        else {
            return Err(RemoteConfigError::NotModified);
        };

        Ok(Self {
            url,
            client,
            registry,
            parser,
            tree,
            refresh_interval: Duration::from_secs(30),
            last_refresh: Instant::now(),
            etag,
            last_modified,
        })
    }

    /// 用 JSON 远程配置构造。
    pub fn json(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser(url, registry, TreeSpec::from_json_str)
    }

    /// 用 TOML 远程配置构造。
    #[cfg(feature = "toml")]
    pub fn toml(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser(url, registry, TreeSpec::from_toml_str)
    }

    /// 用 YAML 远程配置构造。
    #[cfg(feature = "yaml")]
    pub fn yaml(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser(url, registry, TreeSpec::from_yaml_str)
    }

    /// 用 RON 远程配置构造。
    #[cfg(feature = "ron")]
    pub fn ron(
        url: impl Into<String>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, RemoteConfigError> {
        Self::with_parser(url, registry, TreeSpec::from_ron_str)
    }

    /// 设置刷新间隔，默认 30 秒。
    #[must_use]
    pub fn with_refresh_interval(mut self, interval: Duration) -> Self {
        self.refresh_interval = interval;
        self
    }

    /// 远程配置 URL。
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// 当前行为树。
    #[must_use]
    pub fn tree(&self) -> &BehaviorTree<W> {
        &self.tree
    }

    /// 可变行为树。刷新后可用它重新安装 Tracer。
    pub fn tree_mut(&mut self) -> &mut BehaviorTree<W> {
        &mut self.tree
    }

    /// 构建/刷新使用的注册表。
    #[must_use]
    pub fn registry(&self) -> &NodeRegistry<W> {
        &self.registry
    }

    /// 可变注册表；注册新节点后下一次刷新生效。
    pub fn registry_mut(&mut self) -> &mut NodeRegistry<W> {
        &mut self.registry
    }

    /// 按刷新间隔拉取；发生重载返回 `true`，304/未到时间返回 `false`。
    pub fn poll(&mut self) -> Result<bool, RemoteConfigError> {
        if self.last_refresh.elapsed() < self.refresh_interval {
            return Ok(false);
        }
        self.refresh_now()
    }

    /// 立即拉取一次，忽略刷新间隔。
    ///
    /// 304 返回 `false`；成功构建并替换返回 `true`。
    pub fn refresh_now(&mut self) -> Result<bool, RemoteConfigError> {
        self.last_refresh = Instant::now();

        let etag = self.etag.clone();
        let last_modified = self.last_modified.clone();
        let Some((mut new_tree, etag, last_modified)) = fetch(
            &self.client,
            &self.url,
            self.parser,
            &self.registry,
            etag.as_deref(),
            last_modified.as_deref(),
        )?
        else {
            return Ok(false);
        };

        let board = std::mem::take(self.tree.blackboard_mut());
        *new_tree.blackboard_mut() = board;
        self.tree = new_tree;
        self.etag = etag;
        self.last_modified = last_modified;

        Ok(true)
    }

    /// 先按间隔刷新，再 tick 当前树。
    ///
    /// 刷新失败时返回错误且不 tick；需要继续旧树时直接使用
    /// [`RemoteTreeLoader::tree_mut`]。
    pub fn tick(&mut self, world: &mut W, time: Time) -> Result<Status, RemoteConfigError> {
        self.poll()?;
        Ok(self.tree.tick(world, time))
    }
}

fn fetch<W: 'static>(
    client: &Client,
    url: &str,
    parser: TreeParser,
    registry: &NodeRegistry<W>,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Fetched<W>, RemoteConfigError> {
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
    let text = response.text()?;
    let spec = parser(&text)?;
    let tree = spec.build(registry)?;

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

    fn serve_sequence(bodies: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
        let address = listener.local_addr().expect("local address");

        thread::spawn(move || {
            for body in bodies {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
    fn invalid_payload_returns_parse_error() {
        let url = serve_sequence(vec![String::from("{")]);
        let error = RemoteTreeLoader::<()>::json(url, registry())
            .err()
            .expect("parse error");
        assert!(matches!(error, RemoteConfigError::Parse(_)));
    }
}
