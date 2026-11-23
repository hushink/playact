//! An end-to-end reload audit: nested includes, timestamps, revisions, and HTTP.
//!
//! Run:
//!
//! ```text
//! cargo run --example reload_audit --features hot-reload,inspector,diff-serde
//! ```
//!
//! The example creates a temporary configuration with two nested include files,
//! edits the innermost file, and reports how the watcher, diff history, and
//! inspector endpoints observe the change. A stateful `limiter` keeps its ID
//! across reloads, so each report also shows how many nodes restored state.
//! With `PLAYACT_AUDIT_SERVE=1`, it then keeps serving the inspector so you can
//! edit the printed file path yourself.
//!
//! The inspector has no authentication. Use it for local debugging only.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use playact::config::{FsIncludeResolver, NodeRegistry};
use playact::hot_reload::TreeWatcher;
use playact::inspector::InspectorServer;
use playact::prelude::*;

#[derive(Default)]
struct World {
    attacks: u32,
    heavy_attacks: u32,
}

fn registry() -> NodeRegistry<World> {
    let mut registry = NodeRegistry::<World>::new();
    registry
        .action::<()>("attack", |_| {
            move |ctx: &mut Context<'_, World>| {
                ctx.world().attacks = ctx.world().attacks.saturating_add(1);
                Status::Success
            }
        })
        .expect("register attack");
    registry
        .action::<()>("heavy_attack", |_| {
            move |ctx: &mut Context<'_, World>| {
                ctx.world().heavy_attacks = ctx.world().heavy_attacks.saturating_add(1);
                Status::Success
            }
        })
        .expect("register heavy_attack");
    registry
}

fn write_combat(dir: &Path, action: &str) -> io::Result<()> {
    // `attack_limit` is stateful and keeps its progress across reloads;
    // `attack_action` gives the renamed leaf a stable identity in the diff.
    let template = r#"{
        "root": {
            "type": "sequence",
            "children": [
                {"type": "identified", "id": "attack_limit", "child": {
                    "type": "limiter",
                    "max": 1000,
                    "child": {
                        "type": "identified",
                        "id": "attack_action",
                        "child": {"type": "action", "name": "__ACTION__"}
                    }
                }}
            ]
        }
    }"#;
    fs::write(
        dir.join("combat.json"),
        template.replace("__ACTION__", action),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "playact_reload_audit_{}_{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir_all(&dir)?;
    let root = dir.join("root.json");
    let patrol = dir.join("patrol.json");
    let combat = dir.join("combat.json");
    fs::write(&root, r#"{"root":{"type":"include","path":"patrol.json"}}"#)?;
    fs::write(
        &patrol,
        r#"{"root":{"type":"sequence","children":[{"type":"include","path":"combat.json"}]}}"#,
    )?;
    write_combat(&dir, "attack")?;

    let resolver = FsIncludeResolver::json(&dir)?;
    let stats = StatsTracer::new();
    let mut watcher = TreeWatcher::<World>::json_with_resolver(&root, registry(), resolver)?
        .with_debounce(Duration::ZERO)
        .with_state_preservation(true)
        .with_diff_history(8);
    watcher.tree_mut().set_tracer(Some(Box::new(stats.clone())));

    println!("config directory: {}", dir.display());
    let mut includes: Vec<_> = watcher.include_paths().collect();
    includes.sort();
    for path in &includes {
        println!("watching include: {}", path.display());
    }

    let mut world = World::default();
    let mut now_ms = 0_u64;
    let status = watcher
        .tree_mut()
        .tick(&mut world, Time::from_millis(now_ms, 16));
    println!(
        "tick: {status:?}, attacks={}, heavy_attacks={}",
        world.attacks, world.heavy_attacks
    );

    write_combat(&dir, "heavy_attack")?;
    if watcher.poll()? {
        print_reload(&watcher);
    }
    now_ms += 16;
    let status = watcher
        .tree_mut()
        .tick(&mut world, Time::from_millis(now_ms, 16));
    println!(
        "tick: {status:?}, attacks={}, heavy_attacks={}",
        world.attacks, world.heavy_attacks
    );

    write_combat(&dir, "attack")?;
    if watcher.poll()? {
        print_reload(&watcher);
    }

    let mut server = InspectorServer::bind("127.0.0.1:0")?;
    let addr = server.local_addr()?;
    println!("inspector: http://{addr}/");
    let status = request(
        &mut server,
        watcher.tree(),
        &stats,
        watcher.diff_history(),
        watcher.last_reload(),
        addr,
        "/status.json",
    )?;
    println!("/status.json: {status}");
    let incremental = request(
        &mut server,
        watcher.tree(),
        &stats,
        watcher.diff_history(),
        watcher.last_reload(),
        addr,
        "/diffs.json?since=1",
    )?;
    println!("/diffs.json?since=1: {incremental}");

    if std::env::var_os("PLAYACT_AUDIT_SERVE").is_some() {
        println!(
            "serving; edit {} and watch revision/timestamp update (Ctrl+C to stop)",
            combat.display()
        );
        loop {
            match watcher.poll() {
                Ok(true) => print_reload(&watcher),
                Ok(false) => {}
                Err(error) => eprintln!("reload failed: {error}"),
            }
            server.poll_with_report(
                watcher.tree(),
                Some(&stats),
                watcher.diff_history(),
                watcher.last_reload(),
            )?;
            watcher
                .tree_mut()
                .tick(&mut world, Time::from_millis(now_ms, 16));
            now_ms = now_ms.wrapping_add(16);
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fs::remove_dir_all(dir)?;
    Ok(())
}

fn print_reload(watcher: &TreeWatcher<World>) {
    let Some(report) = watcher.last_reload() else {
        return;
    };
    let timestamp = report
        .timestamp
        .map_or_else(|| String::from("unknown"), |unix_ms| unix_ms.to_string());
    println!(
        "reload revision {} (timestamp_ms={}, restored={}):",
        report.revision,
        timestamp,
        report.restored.unwrap_or(0)
    );
    if let Some(error) = report.restore_error {
        println!("restore error: {error}");
    }
    print!("{}", report.diff);
}

fn request(
    server: &mut InspectorServer,
    tree: &BehaviorTree<World>,
    stats: &StatsTracer,
    history: &DiffHistory,
    report: Option<ReloadReport<'_>>,
    addr: SocketAddr,
    path: &str,
) -> io::Result<String> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n")?;

    let served = server.poll_with_report(tree, Some(stats), history, report)?;
    if served != 1 {
        return Err(io::Error::other("expected one handled connection"));
    }

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .trim()
        .to_owned())
}
