//! Self-contained HTML inspection reports and a zero-dependency debug server.
//!
//! Enable the `inspector` feature to render a static page with the tree
//! structure, node IDs, last statuses, tick counts, timings, and errors.
//!
//! [`Inspector`] produces a portable report:
//!
//! ```no_run
//! use playact::inspector::Inspector;
//! use playact::{BehaviorTreeBuilder, StatsTracer, Status, Time};
//!
//! # fn main() -> std::io::Result<()> {
//! let stats = StatsTracer::new();
//! let mut tree = BehaviorTreeBuilder::<()>::new("demo")
//!     .action("work", |_| Status::Success)
//!     .build()
//!     .expect("valid tree");
//! tree.set_tracer(Some(Box::new(stats.clone())));
//!
//! let mut world = ();
//! tree.tick(&mut world, Time::ZERO);
//!
//! Inspector::new(&tree)
//!     .with_stats(&stats)
//!     .write_to("tree.html")
//! # }
//! ```
//!
//! [`InspectorServer`] serves the report from `std::net` without an async
//! runtime:
//!
//! ```no_run
//! use playact::inspector::InspectorServer;
//! use playact::{BehaviorTreeBuilder, Status};
//!
//! # fn main() -> std::io::Result<()> {
//! # let mut tree = BehaviorTreeBuilder::<()>::new("demo")
//! #     .action("work", |_| Status::Success)
//! #     .build()
//! #     .expect("valid tree");
//! let mut server = InspectorServer::bind("127.0.0.1:7878")?;
//! server.poll(&tree, None)?;
//! # Ok(())
//! # }
//! ```
//!
//! Call [`InspectorServer::poll_with_history`] to show every stored reload diff:
//! the page renders a collapsible history and `/diffs.json` serves the whole
//! [`DiffHistory`] as a JSON array when `diff-serde` is enabled.
//!
//! The server is a debug tool: it has no authentication, and the report exposes
//! node names, IDs, and timings. Bind it to a loopback address and never expose
//! it on an untrusted network.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use crate::diff::{DiffHistory, TreeDiff};
use crate::node::Kind;
use crate::stats::{NodeStats, StatsTracer};
use crate::status::Status;
use crate::tree::{BehaviorTree, NodePath, TreeDescription};

/// Maximum number of pending connections handled by one [`InspectorServer::poll`].
const MAX_CONNECTIONS_PER_POLL: usize = 8;
/// Maximum time spent waiting for one request line before dropping the connection.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_millis(25);
/// Maximum request head accepted from a client.
const MAX_REQUEST_BYTES: usize = 4096;
/// Default browser refresh interval used by [`InspectorServer`].
const DEFAULT_REFRESH_SECONDS: u64 = 1;

/// Renders a self-contained HTML report for a behavior tree.
///
/// The report contains the structural tree, optional per-node statistics from a
/// [`StatsTracer`], service descriptions, a statistics table, and Mermaid source.
/// It uses inline CSS only: no fonts, scripts, or network resources are loaded.
pub struct Inspector<'a, W> {
    tree: &'a BehaviorTree<W>,
    stats: Option<&'a StatsTracer>,
    diff: Option<&'a TreeDiff>,
    diff_history: Option<&'a DiffHistory>,
    title: Option<String>,
    refresh_seconds: u64,
}

impl<'a, W> Inspector<'a, W> {
    /// Creates a report renderer for the tree.
    #[must_use]
    pub fn new(tree: &'a BehaviorTree<W>) -> Self {
        Self {
            tree,
            stats: None,
            diff: None,
            diff_history: None,
            title: None,
            refresh_seconds: 0,
        }
    }

    /// Adds per-node statistics from a [`StatsTracer`].
    ///
    /// Install the same tracer on the tree before ticking to see live statuses
    /// and timings.
    #[must_use]
    pub fn with_stats(mut self, stats: &'a StatsTracer) -> Self {
        self.stats = Some(stats);
        self
    }

    /// Adds the structural diff from a recent reload or refresh.
    #[must_use]
    pub fn with_diff(mut self, diff: &'a TreeDiff) -> Self {
        self.diff = Some(diff);
        self
    }

    /// Adds the full reload diff history.
    ///
    /// When set, this replaces the single-diff section in the report.
    #[must_use]
    pub fn with_diff_history(mut self, history: &'a DiffHistory) -> Self {
        self.diff_history = Some(history);
        self
    }

    /// Overrides the page title; defaults to the tree name.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Adds `<meta http-equiv="refresh">` with the given interval.
    ///
    /// `0` disables automatic refresh, which is the default for a static file.
    /// [`InspectorServer`] enables a one-second refresh by default.
    #[must_use]
    pub fn with_refresh_seconds(mut self, seconds: u64) -> Self {
        self.refresh_seconds = seconds;
        self
    }

    /// Renders the report as a complete HTML document.
    #[must_use]
    pub fn render(&self) -> String {
        let description = self.tree.describe();
        let services = self.tree.describe_services();
        let stats = self
            .stats
            .map(|stats| stats.nodes().into_iter().collect::<BTreeMap<_, _>>());
        let title = self
            .title
            .clone()
            .unwrap_or_else(|| String::from(self.tree.name()));
        let diff = match (self.diff_history, self.diff) {
            (Some(history), _) => Some(DiffView::History(history)),
            (None, Some(diff)) => Some(DiffView::Single(diff)),
            (None, None) => None,
        };

        render_report(
            &title,
            self.tree.name(),
            &description,
            &services,
            stats.as_ref(),
            diff,
            self.refresh_seconds,
        )
    }

    /// Writes the rendered report to a file.
    pub fn write_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.render())
    }
}

/// A nonblocking HTTP server for live inspection reports.
///
/// The server uses only `std::net` and has no authentication. It is intended
/// for local debugging: bind it to `127.0.0.1`, run it in a debug build, and do
/// not expose it on an untrusted network.
///
/// Endpoints:
///
/// | Path | Response |
/// |---|---|
/// | `/` | HTML report (auto-refreshes every second by default) |
/// | `/metrics` | `StatsTracer` Prometheus text |
/// | `/tree` | ASCII tree with last statuses; requires a `StatsTracer` |
/// | `/diff.json` | Latest `TreeDiff` JSON; requires `diff-serde` and a diff |
/// | `/diffs.json` | Whole `DiffHistory` JSON array, oldest first; requires `diff-serde` |
/// | `/health` | `ok` |
pub struct InspectorServer {
    listener: TcpListener,
    title: Option<String>,
    refresh_seconds: u64,
}

impl InspectorServer {
    /// Binds the debug server; the listener is set to nonblocking mode.
    pub fn bind(addr: impl ToSocketAddrs) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            title: None,
            refresh_seconds: DEFAULT_REFRESH_SECONDS,
        })
    }

    /// The actual bound address, useful when binding to port `0`.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Overrides the report title; defaults to the tree name.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the HTML refresh interval in seconds; defaults to 1, `0` disables it.
    #[must_use]
    pub fn with_refresh_seconds(mut self, seconds: u64) -> Self {
        self.refresh_seconds = seconds;
        self
    }

    /// Serves all pending requests and returns how many connections were handled.
    ///
    /// Call this from the driver loop, for example once per frame. Each
    /// connection waits at most `REQUEST_READ_TIMEOUT` for its request line, so
    /// a slow client can delay the loop briefly. At most
    /// `MAX_CONNECTIONS_PER_POLL` connections are handled per call.
    ///
    /// One malformed or disconnected client is ignored; listener errors are
    /// returned.
    pub fn poll<W>(
        &mut self,
        tree: &BehaviorTree<W>,
        stats: Option<&StatsTracer>,
    ) -> io::Result<usize> {
        self.poll_with_diff(tree, stats, None)
    }

    /// Like [`InspectorServer::poll`], and includes a structural diff in `/`.
    ///
    /// Pass `watcher.last_diff()` / `loader.last_diff()` to show what changed in
    /// the most recent reload or refresh.
    pub fn poll_with_diff<W>(
        &mut self,
        tree: &BehaviorTree<W>,
        stats: Option<&StatsTracer>,
        diff: Option<&TreeDiff>,
    ) -> io::Result<usize> {
        self.poll_with_views(tree, stats, diff, None)
    }

    /// Like [`InspectorServer::poll`], and shows the full reload diff history.
    ///
    /// Pass `watcher.diff_history()` / `loader.diff_history()`. `/` renders every
    /// stored diff with the newest expanded, and `/diff.json` serves the newest
    /// entry. With `diff-serde`, `/diffs.json` returns the whole history as a
    /// JSON array, oldest first.
    pub fn poll_with_history<W>(
        &mut self,
        tree: &BehaviorTree<W>,
        stats: Option<&StatsTracer>,
        history: &DiffHistory,
    ) -> io::Result<usize> {
        self.poll_with_views(tree, stats, history.latest(), Some(history))
    }

    fn poll_with_views<W>(
        &mut self,
        tree: &BehaviorTree<W>,
        stats: Option<&StatsTracer>,
        diff: Option<&TreeDiff>,
        history: Option<&DiffHistory>,
    ) -> io::Result<usize> {
        let mut served = 0;
        while served < MAX_CONNECTIONS_PER_POLL {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let _ = self.handle(stream, tree, stats, diff, history);
                    served += 1;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(served)
    }

    fn handle<W>(
        &self,
        mut stream: TcpStream,
        tree: &BehaviorTree<W>,
        stats: Option<&StatsTracer>,
        diff: Option<&TreeDiff>,
        history: Option<&DiffHistory>,
    ) -> io::Result<()> {
        stream.set_read_timeout(Some(REQUEST_READ_TIMEOUT))?;
        let mut buffer = [0_u8; MAX_REQUEST_BYTES];
        let read = match stream.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        let request = String::from_utf8_lossy(&buffer[..read]);
        let line = request.lines().next().unwrap_or_default();
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let target = parts.next().unwrap_or("/");
        let path = target.split('?').next().unwrap_or(target);
        let is_get = method.eq_ignore_ascii_case("GET");
        let is_head = method.eq_ignore_ascii_case("HEAD");

        let (status, content_type, body) = match (is_get || is_head, path) {
            (true, "/") => {
                let mut inspector = Inspector::new(tree).with_refresh_seconds(self.refresh_seconds);
                if let Some(stats) = stats {
                    inspector = inspector.with_stats(stats);
                }
                if let Some(history) = history {
                    inspector = inspector.with_diff_history(history);
                } else if let Some(diff) = diff {
                    inspector = inspector.with_diff(diff);
                }
                if let Some(title) = &self.title {
                    inspector = inspector.with_title(title.clone());
                }
                ("200 OK", "text/html; charset=utf-8", inspector.render())
            }
            #[cfg(feature = "diff-serde")]
            (true, "/diff.json") => match diff.or_else(|| history.and_then(DiffHistory::latest)) {
                Some(diff) => ("200 OK", "application/json; charset=utf-8", diff.to_json()),
                None => (
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    String::from("no diff recorded\n"),
                ),
            },
            #[cfg(feature = "diff-serde")]
            (true, "/diffs.json") => match (history, diff) {
                (Some(history), _) => (
                    "200 OK",
                    "application/json; charset=utf-8",
                    history.to_json(),
                ),
                (None, Some(diff)) => (
                    "200 OK",
                    "application/json; charset=utf-8",
                    format!("[{}]", diff.to_json()),
                ),
                (None, None) => (
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    String::from("no diff recorded\n"),
                ),
            },
            (true, "/metrics") => match stats {
                Some(stats) => (
                    "200 OK",
                    "text/plain; charset=utf-8",
                    stats.render_prometheus(),
                ),
                None => (
                    "200 OK",
                    "text/plain; charset=utf-8",
                    String::from("# no StatsTracer installed\n"),
                ),
            },
            (true, "/tree") => match stats {
                Some(stats) => (
                    "200 OK",
                    "text/plain; charset=utf-8",
                    stats.render_tree(tree),
                ),
                None => (
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    String::from("install a StatsTracer to use /tree\n"),
                ),
            },
            (true, "/health") => ("200 OK", "text/plain; charset=utf-8", String::from("ok\n")),
            (true, _) => (
                "404 Not Found",
                "text/plain; charset=utf-8",
                String::from("not found\n"),
            ),
            (false, _) => (
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                String::from("method not allowed\n"),
            ),
        };

        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes())?;
        if !is_head {
            stream.write_all(body.as_bytes())?;
        }
        stream.flush()
    }
}

/// The diff section to render: one diff or the full bounded history.
enum DiffView<'a> {
    Single(&'a TreeDiff),
    History(&'a DiffHistory),
}

fn render_report(
    title: &str,
    tree_name: &str,
    root: &TreeDescription,
    services: &[TreeDescription],
    stats: Option<&BTreeMap<Vec<usize>, NodeStats>>,
    diff: Option<DiffView<'_>>,
    refresh_seconds: u64,
) -> String {
    let nodes = count_nodes(root) + services.iter().map(count_nodes).sum::<usize>();
    let observed = stats.map_or(0, BTreeMap::len);
    let (ticks, errors, aborts) = stats.map_or((0, 0, 0), |stats| {
        stats
            .values()
            .fold((0_u64, 0_u64, 0_u64), |(ticks, errors, aborts), node| {
                (
                    ticks.saturating_add(node.ticks),
                    errors.saturating_add(node.errors),
                    aborts.saturating_add(node.aborts),
                )
            })
    });
    let root_status = stats
        .and_then(|stats| stats.get(&Vec::new()))
        .and_then(|node| node.last_status);

    let mut out = String::with_capacity(16 * 1024);
    out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    if refresh_seconds > 0 {
        let _ = writeln!(
            out,
            "<meta http-equiv=\"refresh\" content=\"{refresh_seconds}\">"
        );
    }
    let _ = writeln!(
        out,
        "<title>{} · playact inspector</title>",
        escape_html(title)
    );
    out.push_str("<style>");
    out.push_str(STYLE);
    out.push_str("</style>\n</head>\n<body>\n<main>\n");

    out.push_str("<header class=\"top\">\n<div>\n");
    out.push_str("<p class=\"eyebrow\">playact inspector</p>\n");
    let _ = writeln!(out, "<h1>{}</h1>", escape_html(title));
    let _ = writeln!(
        out,
        "<p class=\"subtitle\">tree <code>{}</code></p>",
        escape_html(tree_name)
    );
    out.push_str("</div>\n");
    if refresh_seconds > 0 {
        let _ = writeln!(
            out,
            "<span class=\"refresh\">refreshes every {refresh_seconds}s</span>"
        );
    }
    out.push_str("</header>\n");

    out.push_str("<section class=\"cards\">\n");
    write_card(&mut out, "Nodes", &nodes.to_string(), "");
    write_card(&mut out, "Observed", &observed.to_string(), "");
    write_card(&mut out, "Node ticks", &ticks.to_string(), "");
    write_card(&mut out, "Errors", &errors.to_string(), "");
    write_card(&mut out, "Aborts", &aborts.to_string(), "");
    let (status_class, status_label) = status_parts(root_status);
    write_card(&mut out, "Root status", status_label, status_class);
    out.push_str("</section>\n");

    out.push_str("<section>\n<h2>Tree</h2>\n<div class=\"panel tree\">\n<ul>\n");
    let mut path = Vec::new();
    render_tree_node(&mut out, root, stats, &mut path, true);
    out.push_str("</ul>\n</div>\n</section>\n");

    out.push_str("<section>\n<h2>Services</h2>\n");
    if services.is_empty() {
        out.push_str("<p class=\"muted\">No services registered.</p>\n");
    } else {
        out.push_str("<div class=\"panel tree\">\n<ul>\n");
        for service in services {
            let mut path = Vec::new();
            render_tree_node(&mut out, service, stats, &mut path, false);
        }
        out.push_str("</ul>\n</div>\n");
    }
    out.push_str("</section>\n");

    let mut ids = Vec::new();
    let mut stats_path = Vec::new();
    collect_id_entries(
        &mut ids,
        root,
        NodePath::root(),
        stats,
        &mut stats_path,
        true,
    );
    for (index, service) in services.iter().enumerate() {
        let mut service_stats_path = Vec::new();
        collect_id_entries(
            &mut ids,
            service,
            NodePath::service(index),
            stats,
            &mut service_stats_path,
            false,
        );
    }

    out.push_str("<section>\n<h2>Node IDs</h2>\n");
    if ids.is_empty() {
        out.push_str("<p class=\"muted\">No NodeId values assigned.</p>\n");
    } else {
        out.push_str("<div class=\"panel table-wrap\"><table>\n<thead><tr>");
        out.push_str("<th>Path</th><th>ID</th><th>Node</th><th>Kind</th><th>Status</th>");
        out.push_str("</tr></thead>\n<tbody>\n");
        for entry in &ids {
            let (class, label) = status_parts(entry.status);
            let _ = writeln!(
                out,
                "<tr><td><code>{}</code></td><td><code>{}</code></td><td>{}</td>\
                 <td>{}</td><td><span class=\"status {class}\">{label}</span></td></tr>",
                escape_html(&entry.path.to_string()),
                escape_html(&entry.id),
                escape_html(&entry.name),
                escape_html(entry.kind.as_str()),
            );
        }
        out.push_str("</tbody>\n</table></div>\n");
    }
    out.push_str("</section>\n");

    match diff {
        Some(DiffView::History(history)) => render_diff_history(&mut out, history),
        Some(DiffView::Single(diff)) => {
            out.push_str("<section>\n<h2>Reload diff</h2>\n<div class=\"panel\">\n");
            render_diff_body(&mut out, diff, true);
            out.push_str("</div>\n</section>\n");
        }
        None => {}
    }

    out.push_str("<section>\n<h2>Node statistics</h2>\n");
    match stats {
        Some(stats) if !stats.is_empty() => {
            out.push_str("<div class=\"panel table-wrap\"><table>\n<thead><tr>");
            out.push_str(
                "<th>Path</th><th>Node</th><th>Kind</th><th>ID</th><th>Status</th>\
                 <th>Ticks</th><th>OK</th><th>Fail</th><th>Run</th><th>Aborts</th>\
                 <th>Errors</th><th>Avg</th><th>Max</th>",
            );
            out.push_str("</tr></thead>\n<tbody>\n");
            for (path, node) in stats {
                let (class, label) = status_parts(node.last_status);
                let id = node.id.as_ref().map_or("—", |id| id.as_str());
                let _ = writeln!(
                    out,
                    "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td><code>{}</code></td>\
                     <td><span class=\"status {class}\">{label}</span></td>\
                     <td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>\
                     <td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>\
                     <td class=\"num\">{}</td><td class=\"num\">{}</td></tr>",
                    escape_html(&format_path(path)),
                    escape_html(&node.name),
                    escape_html(node.kind.as_str()),
                    escape_html(id),
                    node.ticks,
                    node.successes,
                    node.failures,
                    node.running,
                    node.aborts,
                    node.errors,
                    format_duration(node.average_time()),
                    format_duration(node.max_time),
                );
            }
            out.push_str("</tbody>\n</table></div>\n");
        }
        _ => out.push_str(
            "<p class=\"muted\">No statistics recorded yet. Install a <code>StatsTracer</code> \
             and tick the tree.</p>\n",
        ),
    }
    out.push_str("</section>\n");

    out.push_str("<section>\n<h2>Mermaid</h2>\n<details class=\"panel\"><summary>Show Mermaid source</summary><pre>");
    out.push_str(&escape_html(&root.to_mermaid()));
    out.push_str("</pre></details>\n</section>\n");

    out.push_str("<footer>Read-only debug view generated by playact.</footer>\n");
    out.push_str("</main>\n</body>\n</html>\n");
    out
}

struct IdEntry {
    path: NodePath,
    id: String,
    name: String,
    kind: Kind,
    status: Option<Status>,
}

fn collect_id_entries(
    entries: &mut Vec<IdEntry>,
    node: &TreeDescription,
    path: NodePath,
    stats: Option<&BTreeMap<Vec<usize>, NodeStats>>,
    stats_path: &mut Vec<usize>,
    include_stats: bool,
) {
    if let Some(id) = &node.id {
        let status = if include_stats {
            stats
                .and_then(|stats| stats.get(stats_path))
                .and_then(|node| node.last_status)
        } else {
            None
        };
        entries.push(IdEntry {
            path: path.clone(),
            id: String::from(id.as_str()),
            name: node.name.clone(),
            kind: node.kind,
            status,
        });
    }

    for (index, child) in node.children.iter().enumerate() {
        if include_stats {
            stats_path.push(index);
        }
        collect_id_entries(
            entries,
            child,
            path.with_child(index),
            stats,
            stats_path,
            include_stats,
        );
        if include_stats {
            stats_path.pop();
        }
    }
}

fn render_diff_history(out: &mut String, history: &DiffHistory) {
    out.push_str("<section>\n<h2>Reload history</h2>\n<div class=\"panel\">\n");
    if history.is_empty() {
        out.push_str("<p class=\"muted\">No reload diffs recorded.</p>\n");
    } else {
        let total = history.len();
        for (index, diff) in history.iter().enumerate() {
            let latest = index + 1 == total;
            let open = if latest { " open" } else { "" };
            let marker = if latest { " · latest" } else { "" };
            let _ = writeln!(
                out,
                "<details class=\"history-entry\"{open}><summary>#{} · added {} · \
                 removed {} · moved {} · renamed {}{marker}</summary>",
                index + 1,
                diff.added.len(),
                diff.removed.len(),
                diff.moved.len(),
                diff.renamed.len()
            );
            out.push_str("<div class=\"history-body\">\n");
            render_diff_body(out, diff, false);
            out.push_str("</div>\n</details>\n");
        }
    }
    out.push_str("</div>\n</section>\n");
}

fn render_diff_body(out: &mut String, diff: &TreeDiff, include_counts: bool) {
    if diff.is_empty() {
        out.push_str("<p class=\"muted\">No structural changes.</p>\n");
        return;
    }
    if include_counts {
        let _ = writeln!(
            out,
            "<p class=\"muted\">added {} · removed {} · moved {} · renamed {}</p>",
            diff.added.len(),
            diff.removed.len(),
            diff.moved.len(),
            diff.renamed.len()
        );
    }

    out.push_str("<ul class=\"diff-list\">\n");
    for node in &diff.added {
        write_diff_line(
            out,
            "added",
            "+",
            node.id.as_str(),
            &node.name,
            node.kind,
            &node.path.to_string(),
            None,
        );
    }
    for node in &diff.removed {
        write_diff_line(
            out,
            "removed",
            "-",
            node.id.as_str(),
            &node.name,
            node.kind,
            &node.path.to_string(),
            None,
        );
    }
    for node in &diff.moved {
        write_diff_line(
            out,
            "moved",
            "~",
            node.id.as_str(),
            &node.name,
            node.kind,
            &node.from.to_string(),
            Some(&node.to.to_string()),
        );
    }
    for node in &diff.renamed {
        let _ = writeln!(
            out,
            "<li><span class=\"diff-badge renamed\">*</span>\
             <code>{}</code> <span class=\"kind\">{}</span> {} &rarr; \
             <span class=\"kind\">{}</span> {} at <code>{}</code></li>",
            escape_html(node.id.as_str()),
            escape_html(node.old_kind.as_str()),
            escape_html(&node.old_name),
            escape_html(node.new_kind.as_str()),
            escape_html(&node.new_name),
            escape_html(&node.path.to_string()),
        );
    }
    out.push_str("</ul>\n");
}

#[allow(clippy::too_many_arguments)]
fn write_diff_line(
    out: &mut String,
    class: &str,
    marker: &str,
    id: &str,
    name: &str,
    kind: Kind,
    path: &str,
    to: Option<&str>,
) {
    let _ = write!(
        out,
        "<li><span class=\"diff-badge {class}\">{marker}</span>\
         <code>{}</code> {} <span class=\"kind\">{}</span> at <code>{}</code>",
        escape_html(id),
        escape_html(name),
        escape_html(kind.as_str()),
        escape_html(path)
    );
    if let Some(to) = to {
        let _ = write!(out, " &rarr; <code>{}</code>", escape_html(to));
    }
    out.push_str("</li>\n");
}

fn write_card(out: &mut String, label: &str, value: &str, class: &str) {
    let value_class = if class.is_empty() {
        String::from("card-value")
    } else {
        format!("card-value status {class}")
    };
    let _ = writeln!(
        out,
        "<div class=\"card\"><span class=\"card-label\">{}</span>\
         <span class=\"{}\">{}</span></div>",
        escape_html(label),
        value_class,
        escape_html(value)
    );
}

fn render_tree_node(
    out: &mut String,
    node: &TreeDescription,
    stats: Option<&BTreeMap<Vec<usize>, NodeStats>>,
    path: &mut Vec<usize>,
    include_stats: bool,
) {
    let node_stats = if include_stats {
        stats.and_then(|stats| stats.get(path))
    } else {
        None
    };
    let (class, label) = status_parts(node_stats.and_then(|node| node.last_status));

    out.push_str("<li>");
    if node.children.is_empty() {
        out.push_str("<div class=\"node leaf\">");
        write_node_header(out, node, class, label, node_stats);
        out.push_str("</div>");
    } else {
        out.push_str("<details open><summary class=\"node\">");
        write_node_header(out, node, class, label, node_stats);
        out.push_str("</summary>\n<ul>\n");
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            render_tree_node(out, child, stats, path, include_stats);
            path.pop();
        }
        out.push_str("</ul>\n</details>");
    }
    out.push_str("</li>\n");
}

fn write_node_header(
    out: &mut String,
    node: &TreeDescription,
    class: &'static str,
    label: &'static str,
    stats: Option<&NodeStats>,
) {
    let _ = write!(
        out,
        "<span class=\"status {class}\">{label}</span>\
         <span class=\"kind\">{}</span><strong>{}</strong>",
        escape_html(node.kind.as_str()),
        escape_html(&node.name)
    );
    if let Some(id) = &node.id {
        let _ = write!(
            out,
            "<code class=\"node-id\">{}</code>",
            escape_html(id.as_str())
        );
    }
    if let Some(stats) = stats {
        let _ = write!(
            out,
            "<span class=\"stats\">{} ticks · {} ok · {} fail · {} run",
            stats.ticks, stats.successes, stats.failures, stats.running
        );
        if stats.aborts > 0 {
            let _ = write!(out, " · {} aborts", stats.aborts);
        }
        if stats.errors > 0 {
            let _ = write!(out, " · {} errors", stats.errors);
        }
        let _ = write!(
            out,
            " · avg {} · max {}</span>",
            format_duration(stats.average_time()),
            format_duration(stats.max_time)
        );
    }
}

fn count_nodes(node: &TreeDescription) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

fn format_path(path: &[usize]) -> String {
    if path.is_empty() {
        return String::from("root");
    }
    let mut out = String::new();
    for (index, segment) in path.iter().enumerate() {
        if index > 0 {
            out.push('/');
        }
        let _ = write!(out, "{segment}");
    }
    out
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2} s", duration.as_secs_f64())
    } else if duration.as_millis() > 0 {
        format!("{:.2} ms", duration.as_secs_f64() * 1_000.0)
    } else {
        format!("{:.1} µs", duration.as_secs_f64() * 1_000_000.0)
    }
}

fn status_parts(status: Option<Status>) -> (&'static str, &'static str) {
    match status {
        Some(Status::Success) => ("success", "success"),
        Some(Status::Failure) => ("failure", "failure"),
        Some(Status::Running) => ("running", "running"),
        None => ("unknown", "unknown"),
    }
}

fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #f6f8fa;
  --panel: #ffffff;
  --text: #1f2328;
  --muted: #656d76;
  --border: #d0d7de;
  --accent: #0969da;
  --success: #1a7f37;
  --failure: #cf222e;
  --running: #9a6700;
  --unknown: #6e7781;
  --shadow: 0 1px 2px rgba(31, 35, 40, 0.06);
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0d1117;
    --panel: #161b22;
    --text: #e6edf3;
    --muted: #8b949e;
    --border: #30363d;
    --accent: #4493f8;
    --success: #3fb950;
    --failure: #f85149;
    --running: #d29922;
    --unknown: #8b949e;
    --shadow: 0 1px 2px rgba(1, 4, 9, 0.6);
  }
}
* { box-sizing: border-box; }
body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font-family: ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
  line-height: 1.5;
}
main { max-width: 1100px; margin: 0 auto; padding: 32px 24px 56px; }
header.top {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 24px;
}
.eyebrow {
  margin: 0 0 4px;
  color: var(--muted);
  font-size: 12px;
  letter-spacing: .12em;
  text-transform: uppercase;
}
h1 { margin: 0; font-size: 28px; letter-spacing: -0.02em; }
h2 { margin: 32px 0 12px; font-size: 18px; }
.subtitle { margin: 6px 0 0; color: var(--muted); font-size: 14px; }
.refresh { color: var(--muted); font-size: 12px; white-space: nowrap; }
.cards {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
  gap: 12px;
}
.card {
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 12px 14px;
  box-shadow: var(--shadow);
}
.card-label {
  display: block;
  color: var(--muted);
  font-size: 12px;
  letter-spacing: .06em;
  text-transform: uppercase;
}
.card-value {
  display: block;
  margin-top: 4px;
  font-size: 22px;
  font-weight: 650;
  font-variant-numeric: tabular-nums;
}
.card-value.status { font-size: 18px; }
.panel {
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 16px;
  box-shadow: var(--shadow);
}
.tree ul {
  list-style: none;
  margin: 0;
  padding-left: 18px;
  border-left: 1px solid var(--border);
}
.tree > ul { padding-left: 0; border-left: 0; }
.tree li { margin: 4px 0; }
details > summary { list-style: none; cursor: pointer; }
details > summary::-webkit-details-marker { display: none; }
.tree details > summary::before { content: "▸"; color: var(--muted); }
.tree details[open] > summary::before { content: "▾"; }
.node {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border-radius: 6px;
}
.node:hover { background: color-mix(in srgb, var(--accent) 8%, transparent); }
.node strong { overflow-wrap: anywhere; }
.leaf { padding-left: 8px; }
.kind {
  color: var(--muted);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
}
.node-id {
  padding: 1px 6px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--accent);
  font-size: 12px;
}
.stats {
  flex-basis: 100%;
  color: var(--muted);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
.status {
  padding: 1px 8px;
  border: 1px solid currentColor;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 650;
  letter-spacing: .04em;
  text-transform: uppercase;
}
.status.success { color: var(--success); }
.status.failure { color: var(--failure); }
.status.running { color: var(--running); }
.status.unknown { color: var(--unknown); }
.diff-list { list-style: none; margin: 12px 0 0; padding: 0; }
.diff-list li { padding: 4px 0; border-bottom: 1px solid var(--border); }
.diff-list li:last-child { border-bottom: 0; }
.diff-badge { display: inline-block; width: 1.2em; font-weight: 700; text-align: center; }
.diff-badge.added { color: var(--success); }
.diff-badge.removed { color: var(--failure); }
.diff-badge.moved { color: var(--running); }
.diff-badge.renamed { color: var(--accent); }
.history-entry { padding: 8px 0; border-bottom: 1px solid var(--border); }
.history-entry:last-child { border-bottom: 0; }
.history-entry > summary { font-weight: 600; }
.history-entry > summary::before { content: "▸"; color: var(--muted); margin-right: 6px; }
.history-entry[open] > summary::before { content: "▾"; }
.history-body { padding: 8px 0 0 12px; }
.table-wrap { overflow-x: auto; padding: 0; }
table { width: 100%; border-collapse: collapse; font-size: 13px; }
th, td {
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
  text-align: left;
  white-space: nowrap;
}
th {
  color: var(--muted);
  font-size: 11px;
  letter-spacing: .06em;
  text-transform: uppercase;
}
tr:last-child td { border-bottom: 0; }
td.num { text-align: right; font-variant-numeric: tabular-nums; }
code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.92em;
}
pre {
  margin: 12px 0 0;
  padding: 14px;
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg);
  font-size: 12px;
}
details.panel > summary { font-weight: 600; }
.muted { color: var(--muted); }
footer { margin-top: 40px; color: var(--muted); font-size: 12px; }
@media (max-width: 640px) {
  header.top { align-items: flex-start; flex-direction: column; }
  main { padding: 24px 16px 48px; }
}
"#;

#[cfg(test)]
mod tests {
    use std::net::TcpStream;

    use crate::builder::BehaviorTreeBuilder;
    use crate::time::Time;

    use super::*;

    fn tree(name: &str) -> BehaviorTree<()> {
        BehaviorTreeBuilder::<()>::new(name)
            .sequence(|b| {
                b.condition("ready", |_| true)
                    .id("ready")
                    .action("work", |_| Status::Success)
                    .id("work")
            })
            .build()
            .expect("tree")
    }

    #[test]
    fn renders_structure_status_and_stats() {
        let stats = StatsTracer::new();
        let mut tree = tree("demo");
        tree.set_tracer(Some(Box::new(stats.clone())));
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

        let html = Inspector::new(&tree).with_stats(&stats).render();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("tree <code>demo</code>"));
        assert!(html.contains("sequence"));
        assert!(html.contains("work"));
        assert!(html.contains("Node statistics"));
        assert!(html.contains("status success"));
        assert!(html.contains("ticks"));
        assert!(html.contains("Mermaid"));
        assert!(html.contains("Services"));
        assert!(html.contains("Node IDs"));
        assert!(html.contains("root.children[0]"));
        assert!(html.contains("root.children[1]"));
    }

    #[test]
    fn renders_reload_diff() {
        let old = BehaviorTreeBuilder::<()>::new("old")
            .sequence(|b| b.action("old", |_| Status::Success).id("old"))
            .build()
            .expect("old tree");
        let new = BehaviorTreeBuilder::<()>::new("new")
            .sequence(|b| b.action("new", |_| Status::Success).id("new"))
            .build()
            .expect("new tree");
        let diff = TreeDiff::between(&old, &new);

        let html = Inspector::new(&new).with_diff(&diff).render();
        assert!(html.contains("Reload diff"));
        assert!(html.contains("diff-badge added"));
        assert!(html.contains("diff-badge removed"));
        assert!(html.contains("root.children[0]"));
    }

    #[test]
    fn renders_renamed_entries() {
        let old = BehaviorTreeBuilder::<()>::new("old")
            .sequence(|b| b.action("old", |_| Status::Success).id("same"))
            .build()
            .expect("old tree");
        let new = BehaviorTreeBuilder::<()>::new("new")
            .sequence(|b| b.condition("new", |_| true).id("same"))
            .build()
            .expect("new tree");
        let diff = TreeDiff::between(&old, &new);
        assert_eq!(diff.renamed.len(), 1);

        let html = Inspector::new(&new).with_diff(&diff).render();
        assert!(html.contains("diff-badge renamed"));
        assert!(html.contains("old"));
        assert!(html.contains("new"));
        assert!(html.contains("action"));
        assert!(html.contains("condition"));
        assert!(html.contains("root.children[0]"));
    }

    #[test]
    fn renders_diff_history_with_latest_expanded() {
        let first = BehaviorTreeBuilder::<()>::new("first")
            .sequence(|b| b.action("first", |_| Status::Success).id("first"))
            .build()
            .expect("first tree");
        let second = BehaviorTreeBuilder::<()>::new("second")
            .sequence(|b| {
                b.action("first", |_| Status::Success)
                    .id("first")
                    .action("second", |_| Status::Success)
                    .id("second")
            })
            .build()
            .expect("second tree");

        let mut history = DiffHistory::new(4);
        history.push(TreeDiff::between(&first, &first));
        history.push(TreeDiff::between(&first, &second));

        let html = Inspector::new(&second).with_diff_history(&history).render();
        assert!(html.contains("Reload history"));
        assert!(html.contains("#1 · added 0"));
        assert!(html.contains("#2 · added 1"));
        assert!(html.contains("latest"));
        assert!(html.contains("<details class=\"history-entry\" open>"));
        assert!(html.contains("diff-badge added"));
        assert!(html.contains("second"));
        // The single-diff section is replaced rather than duplicated.
        assert!(!html.contains("<h2>Reload diff</h2>"));
    }

    #[test]
    fn renders_empty_diff_history_placeholder() {
        let empty = tree("empty");
        let history = DiffHistory::new(2);
        let html = Inspector::new(&empty).with_diff_history(&history).render();
        assert!(html.contains("Reload history"));
        assert!(html.contains("No reload diffs recorded."));
    }

    #[test]
    fn renders_node_id_audit_for_services() {
        use crate::leaf::Action;
        use crate::node::IdentifiedNode;

        let mut tree = tree("ids");
        tree.add_service(Box::new(IdentifiedNode::new(
            "sense",
            Box::new(Action::<()>::new("sense", |_| Status::Success)),
        )));

        let html = Inspector::new(&tree).render();
        assert!(html.contains("Node IDs"));
        assert!(html.contains("root.children[0]"));
        assert!(html.contains("root.children[1]"));
        assert!(html.contains("services[0]"));
        assert!(html.contains("sense"));
    }

    #[test]
    fn escapes_html_from_node_and_tree_names() {
        let mut tree = BehaviorTreeBuilder::<()>::new("<script>alert(1)</script>")
            .action("<b>&\"'", |_| Status::Success)
            .build()
            .expect("tree");
        let mut world = ();
        tree.tick(&mut world, Time::ZERO);

        let html = Inspector::new(&tree).with_title("<title>&").render();
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("<b>&\"'"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;b&gt;&amp;&quot;&#39;"));
        assert!(html.contains("&lt;title&gt;&amp;"));
    }

    #[test]
    fn refresh_meta_is_opt_in() {
        let tree = tree("demo");
        let static_html = Inspector::new(&tree).render();
        assert!(!static_html.contains("http-equiv=\"refresh\""));

        let live_html = Inspector::new(&tree).with_refresh_seconds(2).render();
        assert!(live_html.contains("<meta http-equiv=\"refresh\" content=\"2\">"));
        assert!(live_html.contains("refreshes every 2s"));
    }

    #[test]
    fn writes_a_report_file() {
        let path = std::env::temp_dir().join(format!(
            "playact_inspector_{}_{}.html",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let tree = tree("demo");
        Inspector::new(&tree).write_to(&path).expect("write report");
        let contents = std::fs::read_to_string(&path).expect("read report");
        assert!(contents.contains("demo"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn server_serves_report_metrics_and_errors() {
        let stats = StatsTracer::new();
        let mut tree = tree("server");
        tree.set_tracer(Some(Box::new(stats.clone())));
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

        let mut server = InspectorServer::bind("127.0.0.1:0").expect("bind");
        let addr = server.local_addr().expect("addr");
        assert_eq!(server.poll(&tree, Some(&stats)).expect("idle poll"), 0);

        let html = request(
            &mut server,
            &tree,
            Some(&stats),
            addr,
            "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(html.starts_with("HTTP/1.1 200 OK"));
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("server"));
        assert!(html.contains("refresh"));

        let metrics = request(
            &mut server,
            &tree,
            Some(&stats),
            addr,
            "GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(metrics.starts_with("HTTP/1.1 200 OK"));
        assert!(metrics.contains("playact_node_ticks"));

        let tree_text = request(
            &mut server,
            &tree,
            Some(&stats),
            addr,
            "GET /tree HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(tree_text.contains("sequence \"Sequence\" [success]"));

        let missing = request(
            &mut server,
            &tree,
            Some(&stats),
            addr,
            "GET /missing HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(missing.starts_with("HTTP/1.1 404 Not Found"));

        let post = request(
            &mut server,
            &tree,
            Some(&stats),
            addr,
            "POST / HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(post.starts_with("HTTP/1.1 405 Method Not Allowed"));
    }

    #[cfg(feature = "diff-serde")]
    #[test]
    fn server_serves_diff_json() {
        let old = BehaviorTreeBuilder::<()>::new("old")
            .sequence(|b| b.action("old", |_| Status::Success).id("old"))
            .build()
            .expect("old tree");
        let new = BehaviorTreeBuilder::<()>::new("new")
            .sequence(|b| b.action("new", |_| Status::Success).id("new"))
            .build()
            .expect("new tree");
        let diff = TreeDiff::between(&old, &new);

        let mut server = InspectorServer::bind("127.0.0.1:0").expect("bind");
        let addr = server.local_addr().expect("addr");
        let response = request_with_diff(
            &mut server,
            &new,
            None,
            Some(&diff),
            addr,
            "GET /diff.json HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("application/json"));
        assert!(response.contains("\"added\""));
        assert!(response.contains("\"id\":\"new\""));
    }

    #[cfg(feature = "diff-serde")]
    #[test]
    fn server_serves_diff_history() {
        let first = BehaviorTreeBuilder::<()>::new("first")
            .sequence(|b| b.action("first", |_| Status::Success).id("first"))
            .build()
            .expect("first tree");
        let second = BehaviorTreeBuilder::<()>::new("second")
            .sequence(|b| {
                b.action("first", |_| Status::Success)
                    .id("first")
                    .action("second", |_| Status::Success)
                    .id("second")
            })
            .build()
            .expect("second tree");

        let mut history = DiffHistory::new(4);
        history.push(TreeDiff::between(&first, &first));
        history.push(TreeDiff::between(&first, &second));

        let mut server = InspectorServer::bind("127.0.0.1:0").expect("bind");
        let addr = server.local_addr().expect("addr");

        let response = request_with_history(
            &mut server,
            &second,
            None,
            &history,
            addr,
            "GET /diffs.json HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("application/json"));
        let body = response.split("\r\n\r\n").nth(1).expect("body");
        let value: serde_json::Value = serde_json::from_str(body).expect("history json");
        let entries = value.as_array().expect("array");
        assert_eq!(entries.len(), 2);
        assert!(entries[0]["added"].as_array().expect("added").is_empty());
        assert_eq!(entries[1]["added"][0]["id"], "second");

        let response = request_with_history(
            &mut server,
            &second,
            None,
            &history,
            addr,
            "GET /diff.json HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        let body = response.split("\r\n\r\n").nth(1).expect("body");
        let value: serde_json::Value = serde_json::from_str(body).expect("latest json");
        assert_eq!(value["added"][0]["id"], "second");
    }

    fn request<W>(
        server: &mut InspectorServer,
        tree: &BehaviorTree<W>,
        stats: Option<&StatsTracer>,
        addr: SocketAddr,
        request: &str,
    ) -> String {
        request_with_diff(server, tree, stats, None, addr, request)
    }

    fn request_with_diff<W>(
        server: &mut InspectorServer,
        tree: &BehaviorTree<W>,
        stats: Option<&StatsTracer>,
        diff: Option<&TreeDiff>,
        addr: SocketAddr,
        request: &str,
    ) -> String {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        stream.write_all(request.as_bytes()).expect("write request");
        assert_eq!(server.poll_with_diff(tree, stats, diff).expect("poll"), 1);
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    }

    fn request_with_history<W>(
        server: &mut InspectorServer,
        tree: &BehaviorTree<W>,
        stats: Option<&StatsTracer>,
        history: &DiffHistory,
        addr: SocketAddr,
        request: &str,
    ) -> String {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        stream.write_all(request.as_bytes()).expect("write request");
        assert_eq!(
            server
                .poll_with_history(tree, stats, history)
                .expect("poll"),
            1
        );
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    }
}
