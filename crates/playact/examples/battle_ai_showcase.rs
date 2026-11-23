//! A fully config-driven battle AI showcase.
//!
//! Run:
//!
//! ```text
//! cargo run -p playact --example battle_ai_showcase --features hot-reload
//! ```
//!
//! The example writes a temporary JSON configuration, watches it, and applies a
//! second configuration halfway through the run. It demonstrates:
//! - `TreeSpec.services` for a sensing service;
//! - `utility_selector` branches driven by typed score functions;
//! - `guard` with a registered predicate;
//! - `try_action` errors reported through `StatsTracer`;
//! - `wait_for_event` consuming service-published events;
//! - state-preserving hot reload with stable `NodeId`s;
//! - a structural `TreeDiff` after the reload.

use serde::Deserialize;

use playact::config::NodeRegistry;
use playact::hot_reload::TreeWatcher;
use playact::prelude::*;

struct World {
    player_hp: f64,
    enemy_hp: f64,
    distance: f64,
    turn: u32,
}

impl World {
    fn new() -> Self {
        Self {
            player_hp: 60.0,
            enemy_hp: 80.0,
            distance: 20.0,
            turn: 0,
        }
    }
}

static TARGET_VISIBLE: Key<bool> = Key::new("showcase.target_visible");
static SCAN_PULSE: EventKey<u32> = EventKey::new("showcase.scan_pulse");

#[derive(Deserialize)]
struct HpBelow {
    ratio: f64,
}

#[derive(Deserialize)]
struct AttackParams {
    damage: f64,
}

#[derive(Deserialize)]
struct HealParams {
    amount: f64,
}

#[derive(Deserialize)]
struct HealScore {
    threshold: f64,
    weight: f32,
}

#[derive(Deserialize)]
struct AttackScore {
    base: f32,
    finish_bonus: f32,
}

#[derive(Deserialize)]
struct IdleScore {
    base: f32,
}

fn build_registry() -> NodeRegistry<World> {
    let mut registry = NodeRegistry::<World>::new();

    registry
        .condition::<HpBelow>("hp_below", |params: HpBelow| {
            move |ctx: &Context<'_, World>| ctx.world_ref().player_hp / 100.0 < params.ratio
        })
        .expect("register hp_below");
    registry
        .condition::<()>("target_visible", |_| {
            move |ctx: &Context<'_, World>| {
                ctx.blackboard_ref()
                    .get(&TARGET_VISIBLE)
                    .copied()
                    .unwrap_or(false)
            }
        })
        .expect("register target_visible");

    registry
        .action::<()>("sense", |_| {
            move |ctx: &mut Context<'_, World>| {
                let visible = ctx.world_ref().distance <= 10.0;
                ctx.blackboard().set(&TARGET_VISIBLE, visible);
                let turn = ctx.world_ref().turn;
                if !visible && turn.is_multiple_of(3) {
                    SCAN_PULSE.push(ctx, turn);
                }
                Status::Success
            }
        })
        .expect("register sense");
    registry
        .action::<()>("scan_extra", |_| {
            move |_: &mut Context<'_, World>| Status::Success
        })
        .expect("register scan_extra");
    registry
        .event_key(SCAN_PULSE)
        .expect("register scan_pulse event");

    registry
        .try_action::<AttackParams, &'static str>("attack", |params: AttackParams| {
            move |ctx: &mut Context<'_, World>| {
                let turn = ctx.world_ref().turn;
                if turn % 4 == 3 {
                    return Err("attack jammed");
                }
                let world = ctx.world();
                if world.enemy_hp > 0.0 {
                    world.enemy_hp = (world.enemy_hp - params.damage).max(0.0);
                }
                Ok(Status::Success)
            }
        })
        .expect("register attack");
    registry
        .try_action::<HealParams, &'static str>("heal", |params: HealParams| {
            move |ctx: &mut Context<'_, World>| {
                let turn = ctx.world_ref().turn;
                if turn % 5 == 4 {
                    return Err("heal fumbled");
                }
                let world = ctx.world();
                world.player_hp = (world.player_hp + params.amount).min(100.0);
                Ok(Status::Success)
            }
        })
        .expect("register heal");

    registry
        .score::<HealScore>("heal_urgency", |params: HealScore| {
            move |ctx: &Context<'_, World>| {
                let hp = ctx.world_ref().player_hp / 100.0;
                if hp < params.threshold {
                    let urgency = ((params.threshold - hp) / params.threshold) as f32;
                    params.weight * (1.0 + urgency)
                } else {
                    0.0
                }
            }
        })
        .expect("register heal_urgency");
    registry
        .score::<AttackScore>("attack_value", |params: AttackScore| {
            move |ctx: &Context<'_, World>| {
                let visible = ctx
                    .blackboard_ref()
                    .get(&TARGET_VISIBLE)
                    .copied()
                    .unwrap_or(false);
                if visible {
                    let enemy = (ctx.world_ref().enemy_hp / 100.0).clamp(0.0, 1.0) as f32;
                    params.base + params.finish_bonus * (1.0 - enemy)
                } else {
                    0.0
                }
            }
        })
        .expect("register attack_value");
    registry
        .score::<IdleScore>("idle_value", |params: IdleScore| {
            move |ctx: &Context<'_, World>| {
                let visible = ctx
                    .blackboard_ref()
                    .get(&TARGET_VISIBLE)
                    .copied()
                    .unwrap_or(false);
                if visible { 0.0 } else { params.base }
            }
        })
        .expect("register idle_value");

    registry
}

const CONFIG_V1: &str = include_str!("battle_ai_showcase.json");

const CONFIG_V2: &str = include_str!("battle_ai_showcase_v2.json");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "playact_battle_ai_showcase_{}.json",
        std::process::id()
    ));
    std::fs::write(&path, CONFIG_V1)?;

    let stats = StatsTracer::new();
    let mut watcher =
        TreeWatcher::<World>::json(&path, build_registry())?.with_state_preservation(true);
    watcher.tree_mut().set_tracer(Some(Box::new(stats.clone())));

    println!("config: {}", path.display());
    println!("services: {:?}", service_names(watcher.tree()));
    println!("\n{}", watcher.tree().describe());

    let mut world = World::new();

    for tick in 0..20_u64 {
        if tick == 10 {
            std::fs::write(&path, CONFIG_V2)?;
            watcher.reload_now()?;
            stats.clear();
            watcher.tree_mut().set_tracer(Some(Box::new(stats.clone())));
            println!(
                "\n-- hot reload: {} (services {:?}) --\n",
                watcher.tree().name(),
                service_names(watcher.tree())
            );
            if let Some(diff) = watcher.last_diff() {
                println!("structural diff:\n{diff}");
            }
        }

        world.turn = tick as u32;
        let status = watcher
            .tree_mut()
            .tick(&mut world, Time::from_millis(tick * 1000, 1000));
        let visible = watcher
            .tree()
            .blackboard()
            .get(&TARGET_VISIBLE)
            .copied()
            .unwrap_or(false);
        advance(&mut world);

        println!(
            "tick {tick:02}: {status:7} hp={:3.0} enemy={:3.0} dist={:2.0} visible={visible}",
            world.player_hp, world.enemy_hp, world.distance
        );
    }

    let errors: u64 = stats.nodes().iter().map(|(_, node)| node.errors).sum();
    println!("\npost-reload try_action errors: {errors}");
    println!("\n{}", stats.render_tree(watcher.tree()));
    println!(
        "\nfinal: player={:.0} enemy={:.0} distance={:.0}",
        world.player_hp, world.enemy_hp, world.distance
    );

    drop(watcher);
    let _ = std::fs::remove_file(path);
    Ok(())
}

fn service_names<W>(tree: &BehaviorTree<W>) -> Vec<String> {
    tree.describe_services()
        .into_iter()
        .map(|service| service.name)
        .collect()
}

fn advance(world: &mut World) {
    world.turn = world.turn.wrapping_add(1);
    world.distance = (world.distance - 2.0).max(4.0);
    if world.enemy_hp > 0.0 {
        world.player_hp = (world.player_hp - 5.0).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use playact::config::TreeSpec;

    use super::*;

    #[test]
    fn showcase_configs_build_with_expected_services() {
        let registry = build_registry();

        let v1 = TreeSpec::from_json_str(CONFIG_V1)
            .expect("v1 spec")
            .build(&registry)
            .expect("v1 tree");
        assert_eq!(v1.service_count(), 1);
        assert_eq!(v1.duplicate_node_id(), None);

        let v2 = TreeSpec::from_json_str(CONFIG_V2)
            .expect("v2 spec")
            .build(&registry)
            .expect("v2 tree");
        assert_eq!(v2.service_count(), 2);
        assert_eq!(v2.duplicate_node_id(), None);

        let diff = TreeDiff::between(&v1, &v2);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].id.as_str(), "scan_extra");
        assert_eq!(diff.added[0].path.to_string(), "services[1]");
        assert!(diff.removed.is_empty());
        assert!(diff.moved.is_empty());
    }

    #[test]
    fn showcase_scenario_reports_try_action_errors() {
        let registry = build_registry();
        let stats = StatsTracer::new();
        let mut world = World::new();

        let mut tree = TreeSpec::from_json_str(CONFIG_V1)
            .expect("v1 spec")
            .build(&registry)
            .expect("v1 tree");
        tree.set_tracer(Some(Box::new(stats.clone())));
        for tick in 0..10_u64 {
            world.turn = tick as u32;
            let _ = tree.tick(&mut world, Time::from_millis(tick * 1000, 1000));
            advance(&mut world);
        }

        let mut tree = TreeSpec::from_json_str(CONFIG_V2)
            .expect("v2 spec")
            .build(&registry)
            .expect("v2 tree");
        tree.set_tracer(Some(Box::new(stats.clone())));
        for tick in 10..20_u64 {
            world.turn = tick as u32;
            let _ = tree.tick(&mut world, Time::from_millis(tick * 1000, 1000));
            advance(&mut world);
        }

        let errors: u64 = stats.nodes().iter().map(|(_, node)| node.errors).sum();
        assert!(errors > 0, "expected try_action errors, got {errors}");
        assert!(world.enemy_hp < 80.0);
        assert!(world.player_hp > 0.0);
    }
}
