//! Config-driven combat example: JSON + NodeRegistry + typed params.
//!
//! Run:
//!
//! ```text
//! cargo run --example battle_ai_config --features config
//! ```
//!
//! Demonstrates:
//! - building a behavior tree from JSON with `TreeSpec`;
//! - action/condition params deserialized into concrete types at build time;
//! - a custom node factory creating a node with internal state;
//! - the `ConfigBuildError` path for an unknown action.

use std::num::NonZeroU32;

use serde::Deserialize;

use playact::config::{NodeRegistry, TreeSpec};
use playact::prelude::*;

// ==================== Domain world (example only) ====================

#[derive(Debug, Clone, Copy, PartialEq)]
enum DuelCommand {
    Attack(f64),
    Heal(f64),
}

struct DuelWorld {
    player_hp: f64,
    enemy_hp: f64,
    turn: u32,
    log: Vec<String>,
}

impl DuelWorld {
    fn new() -> Self {
        Self {
            player_hp: 50.0,
            enemy_hp: 36.0,
            turn: 0,
            log: Vec::new(),
        }
    }

    fn apply(&mut self, command: DuelCommand) {
        match command {
            DuelCommand::Attack(damage) => {
                if self.enemy_hp > 0.0 {
                    self.enemy_hp = (self.enemy_hp - damage).max(0.0);
                    self.log.push(format!("attacks enemy (-{damage:.0})"));
                }
            }
            DuelCommand::Heal(amount) => {
                self.player_hp = (self.player_hp + amount).min(100.0);
                self.log.push(format!("heals self (+{amount:.0})"));
            }
        }
    }

    fn enemy_retaliate(&mut self) {
        if self.enemy_hp > 0.0 && self.player_hp > 0.0 {
            self.player_hp = (self.player_hp - 6.0).max(0.0);
            self.log.push(String::from("enemy retaliates (-6)"));
        }
    }
}

static COMMANDS: Key<CommandBuffer<DuelCommand>> = playact::key!("commands");

fn push_command(ctx: &mut Context<'_, DuelWorld>, command: DuelCommand) {
    ctx.blackboard()
        .get_or_insert_with(&COMMANDS, CommandBuffer::new)
        .push(command);
}

// ==================== typed params ====================

#[derive(Deserialize)]
struct HpBelow {
    ratio: f64,
}

#[derive(Deserialize)]
struct HealParams {
    amount: f64,
}

#[derive(Deserialize)]
struct MultiStrikeParams {
    damage: f64,
    hits: NonZeroU32,
}

// ==================== Custom stateful node ====================

struct MultiStrike {
    damage: f64,
    initial: u32,
    remaining: u32,
}

impl MultiStrike {
    fn new(damage: f64, hits: u32) -> Self {
        Self {
            damage,
            initial: hits,
            remaining: hits,
        }
    }
}

impl Node<DuelWorld> for MultiStrike {
    fn tick(&mut self, ctx: &mut Context<'_, DuelWorld>) -> Status {
        if self.remaining == 0 {
            self.remaining = self.initial;
            return Status::Success;
        }

        push_command(ctx, DuelCommand::Attack(self.damage));
        self.remaining -= 1;

        if self.remaining == 0 {
            // Terminal self-reset contract: restore a restartable state before returning Success.
            self.remaining = self.initial;
            Status::Success
        } else {
            Status::Running
        }
    }

    fn reset(&mut self) {
        self.remaining = self.initial;
    }

    fn name(&self) -> &str {
        "multi_strike"
    }

    fn kind(&self) -> Kind {
        Kind::Custom("multi_strike")
    }
}

// ==================== registry ====================

fn build_registry() -> NodeRegistry<DuelWorld> {
    let mut registry = NodeRegistry::<DuelWorld>::new();

    registry
        .condition::<HpBelow>("hp_below", |params: HpBelow| {
            move |ctx: &Context<'_, DuelWorld>| ctx.world_ref().player_hp / 100.0 < params.ratio
        })
        .expect("register hp_below");

    registry
        .condition::<()>("enemy_alive", |_params: ()| {
            move |ctx: &Context<'_, DuelWorld>| ctx.world_ref().enemy_hp > 0.0
        })
        .expect("register enemy_alive");

    registry
        .action::<HealParams>("heal", |params: HealParams| {
            move |ctx: &mut Context<'_, DuelWorld>| {
                push_command(ctx, DuelCommand::Heal(params.amount));
                Status::Success
            }
        })
        .expect("register heal");

    registry
        .action::<()>("wait", |_params: ()| {
            move |_: &mut Context<'_, DuelWorld>| Status::Running
        })
        .expect("register wait");

    registry
        .custom::<MultiStrikeParams>(
            "multi_strike",
            |params: MultiStrikeParams, _children: Vec<BoxedNode<DuelWorld>>| {
                Ok(Box::new(MultiStrike::new(params.damage, params.hits.get())))
            },
        )
        .expect("register multi_strike");

    registry
}

// ==================== Driver ====================

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = build_registry();
    let spec = TreeSpec::from_json_str(include_str!("battle_ai.json"))?;
    let mut tree = spec.build(&registry)?;
    tree.blackboard_mut().set(&COMMANDS, CommandBuffer::new());

    println!("Configuration-driven behavior tree:\n{}\n", tree.describe());

    let mut world = DuelWorld::new();
    let mut executed_ticks = 0_u32;

    for tick in 0..12_u64 {
        world.turn = tick as u32;
        let time = Time::from_millis(tick * 1000, 1000);
        let status = tree.tick(&mut world, time);

        let commands: Vec<_> = match tree.blackboard_mut().get_mut(&COMMANDS) {
            Some(buffer) => buffer.drain().collect(),
            None => Vec::new(),
        };
        let command_count = commands.len();
        for command in commands {
            world.apply(command);
        }
        world.enemy_retaliate();

        executed_ticks += 1;

        println!(
            "tick {tick:02}: status={status} player={:.0} enemy={:.0} commands={command_count}",
            world.player_hp, world.enemy_hp
        );
        for line in &world.log {
            println!("  · {line}");
        }
        world.log.clear();

        if world.enemy_hp <= 0.0 || world.player_hp <= 0.0 {
            break;
        }
    }

    println!(
        "\nResult: player={:.0} enemy={:.0} ticks={}",
        world.player_hp, world.enemy_hp, executed_ticks
    );

    // Demonstrate the path-carrying error for an unknown action.
    let bad = TreeSpec::from_json_str(
        r#"{"root":{"type":"sequence","children":[{"type":"action","name":"missing"}]}}"#,
    )?;
    match bad.build(&registry) {
        Ok(_) => println!("unexpected: bad spec built successfully"),
        Err(error) => println!("expected build error: {error}"),
    }

    Ok(())
}
