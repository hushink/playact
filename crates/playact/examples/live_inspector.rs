//! A live HTML inspector served over local HTTP using only the standard library.
//!
//! Run:
//!
//! ```text
//! cargo run --example live_inspector --features inspector
//! ```
//!
//! Then open <http://127.0.0.1:7878/>. The page refreshes every second and shows
//! the tree, live statuses, tick counts, and timings.
//!
//! Other endpoints:
//! - `/metrics`: Prometheus text from `StatsTracer`
//! - `/tree`: ASCII tree with last statuses
//! - `/health`: readiness probe
//!
//! Set `PLAYACT_INSPECTOR_ADDR` to change the bind address. The server is for
//! local debugging only: it has no authentication and exposes tree metadata.

use std::io;
use std::time::Duration;

use playact::inspector::InspectorServer;
use playact::prelude::*;

struct World {
    health: u32,
    enemy_near: bool,
    frame: u64,
    attacks: u32,
}

fn main() -> io::Result<()> {
    let mut world = World {
        health: 72,
        enemy_near: false,
        frame: 0,
        attacks: 0,
    };

    let stats = StatsTracer::new();
    let mut tree = BehaviorTreeBuilder::<World>::new("live inspector demo")
        .reactive_selector(|b| {
            b.sequence(|b| {
                b.condition("low health", |ctx| ctx.world_ref().health < 40)
                    .id("low_health")
                    .action("heal", |ctx| {
                        let world = ctx.world();
                        world.health = world.health.saturating_add(3).min(100);
                        Status::Success
                    })
                    .id("heal")
            })
            .sequence(|b| {
                b.condition("enemy near", |ctx| ctx.world_ref().enemy_near)
                    .id("enemy_near")
                    .cooldown(Duration::from_millis(600), |b| {
                        b.action("attack", |ctx| {
                            let world = ctx.world();
                            world.attacks = world.attacks.saturating_add(1);
                            Status::Success
                        })
                        .id("attack")
                    })
                    .id("attack_cooldown")
            })
            .action("idle", |_| Status::Running)
            .id("idle")
        })
        .build()
        .expect("valid tree");
    tree.set_tracer(Some(Box::new(stats.clone())));

    let addr =
        std::env::var("PLAYACT_INSPECTOR_ADDR").unwrap_or_else(|_| String::from("127.0.0.1:7878"));
    let mut server = InspectorServer::bind(&addr)?.with_title("playact live inspector demo");

    println!("playact inspector: http://{}/", server.local_addr()?);
    println!("endpoints: /, /metrics, /tree, /health");
    println!("set PLAYACT_INSPECTOR_ADDR to change the address; Ctrl+C to stop");

    let mut now_ms = 0_u64;
    loop {
        let _ = server.poll(&tree, Some(&stats))?;
        advance_world(&mut world);
        tree.tick(&mut world, Time::from_millis(now_ms, 16));
        now_ms = now_ms.wrapping_add(16);
        std::thread::sleep(Duration::from_millis(16));
    }
}

fn advance_world(world: &mut World) {
    world.frame = world.frame.wrapping_add(1);
    if world.frame.is_multiple_of(120) {
        world.enemy_near = !world.enemy_near;
    }
    if world.frame.is_multiple_of(90) {
        world.health = world.health.saturating_sub(2);
    }
}
