//! Save and restore a behavior tree across a process boundary.
//!
//! Run:
//!
//! ```text
//! cargo run --example save_game --features blackboard-serde,snapshot-serde
//! ```
//!
//! The example runs a stateful tree, persists its node snapshot and typed
//! blackboard to JSON files, rebuilds the tree, restores both, and verifies that
//! the resumed tree continues from the saved progress without re-running the
//! completed step. World state is application-specific and is not part of
//! either snapshot.

use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use playact::prelude::*;

static SCORE: Key<u32> = Key::new("save_game.score");

#[derive(Default)]
struct World {
    hits: u32,
}

fn build_tree() -> BehaviorTree<World> {
    BehaviorTreeBuilder::<World>::new("save-game")
        .sequence(|b| {
            b.action("hit", |ctx| {
                ctx.world().hits = ctx.world().hits.saturating_add(1);
                Status::Success
            })
            .id("hit")
            .wait(Duration::from_secs(1))
            .id("wait")
        })
        .build()
        .expect("valid tree")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "playact_save_game_{}_{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir_all(&dir)?;
    let snapshot_path = dir.join("snapshot.json");
    let blackboard_path = dir.join("blackboard.json");

    let mut tree = build_tree();
    let mut world = World::default();
    tree.blackboard_mut().set(&SCORE, 42);

    // The first step succeeds; the wait starts and keeps the sequence Running.
    assert_eq!(
        tree.tick(&mut world, Time::from_millis(0, 500)),
        Status::Running
    );
    assert_eq!(
        tree.tick(&mut world, Time::from_millis(500, 500)),
        Status::Running
    );

    let snapshot = tree.snapshot();
    let mut codec = BlackboardCodec::new();
    codec.register(&SCORE)?;
    let board_bytes = codec.encode(tree.blackboard())?;

    fs::write(&snapshot_path, snapshot.to_json())?;
    fs::write(&blackboard_path, &board_bytes)?;
    println!(
        "saved {} bytes of node state and {} bytes of blackboard; hits={}",
        fs::metadata(&snapshot_path)?.len(),
        board_bytes.len(),
        world.hits
    );

    // Rebuild the tree and restore both the node snapshot and the blackboard.
    let snapshot_text = fs::read_to_string(&snapshot_path)?;
    let snapshot = TreeSnapshot::from_json(&snapshot_text)?;
    let board_bytes = fs::read(&blackboard_path)?;

    let mut restored = build_tree();
    restored.restore(&snapshot)?;
    codec.decode(restored.blackboard_mut(), &board_bytes)?;

    let mut world = World::default();
    let status = restored.tick(&mut world, Time::from_millis(1_000, 500));
    println!(
        "restored status={status:?} hits={} score={:?}",
        world.hits,
        restored.blackboard().get(&SCORE)
    );

    assert_eq!(status, Status::Success);
    assert_eq!(world.hits, 0, "the completed hit step must not re-run");
    assert_eq!(restored.blackboard().get(&SCORE), Some(&42));

    fs::remove_dir_all(dir)?;
    Ok(())
}
