//! A complete, runnable combat AI example: a player bot vs three goblins.
//!
//! Run:
//!
//! ```text
//! cargo run --example battle_ai
//! ```
//!
//! Highlights:
//! - 1D battlefield, fixed 0.25s logic frames, no randomness: fully deterministic;
//! - the player uses a ReactiveSelector to preempt combat and survive at low HP;
//! - one independent behavior tree and blackboard per agent;
//! - actions only read the world and push BattleCommands into a CommandBuffer on the
//!   blackboard; the main loop executes all commands after every agent has ticked,
//!   demonstrating decision/execution separation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use playact::Kind;
use playact::prelude::*;

// ==================== Domain types (example only) ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entity {
    Player,
    Goblin(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BattleCommand {
    MoveTo { actor: Entity, destination: f64 },
    MoveAway { actor: Entity, from: f64 },
    Attack { actor: Entity, target: Entity },
    CastSkill { actor: Entity, target: Entity },
    DrinkPotion { actor: Entity },
    Patrol { actor: Entity },
}

#[derive(Debug, Clone)]
struct Combatant {
    name: String,
    hp: f64,
    max_hp: f64,
    pos: f64,
    speed: f64,
    range: f64,
    damage: f64,
    potions: u32,
    potion_heal: f64,
    alive: bool,
}

impl Combatant {
    fn hp_ratio(&self) -> f64 {
        (self.hp / self.max_hp).clamp(0.0, 1.0)
    }
}

struct World {
    player: Combatant,
    goblins: Vec<Combatant>,
    dt: Duration,
    tick: u64,
    log: Vec<String>,
}

impl World {
    fn new() -> Self {
        let player = Combatant {
            name: String::from("player"),
            hp: 55.0,
            max_hp: 100.0,
            pos: 0.0,
            speed: 6.0,
            range: 3.5,
            damage: 12.0,
            potions: 2,
            potion_heal: 35.0,
            alive: true,
        };

        let goblins = (0..3)
            .map(|index| Combatant {
                name: format!("goblin#{index}"),
                hp: 40.0,
                max_hp: 40.0,
                pos: 9.0 + index as f64 * 4.0,
                speed: 4.0,
                range: 2.5,
                damage: 8.0,
                potions: 0,
                potion_heal: 0.0,
                alive: true,
            })
            .collect();

        Self {
            player,
            goblins,
            dt: Duration::from_millis(250),
            tick: 0,
            log: Vec::new(),
        }
    }

    fn combatant(&self, entity: Entity) -> Option<&Combatant> {
        match entity {
            Entity::Player => Some(&self.player),
            Entity::Goblin(index) => self.goblins.get(index),
        }
    }

    fn combatant_mut(&mut self, entity: Entity) -> Option<&mut Combatant> {
        match entity {
            Entity::Player => Some(&mut self.player),
            Entity::Goblin(index) => self.goblins.get_mut(index),
        }
    }

    fn nearest_goblin(&self) -> Option<(usize, f64)> {
        self.goblins
            .iter()
            .enumerate()
            .filter(|(_, goblin)| goblin.alive)
            .map(|(index, goblin)| (index, (goblin.pos - self.player.pos).abs()))
            .min_by(|left, right| left.1.total_cmp(&right.1))
    }

    fn apply(&mut self, command: BattleCommand) {
        match command {
            BattleCommand::MoveTo { actor, destination } => {
                self.move_toward(actor, destination);
            }
            BattleCommand::MoveAway { actor, from } => {
                self.move_away(actor, from);
            }
            BattleCommand::Attack { actor, target } => {
                self.strike(actor, target, false);
            }
            BattleCommand::CastSkill { actor, target } => {
                self.strike(actor, target, true);
            }
            BattleCommand::DrinkPotion { actor } => {
                self.drink(actor);
            }
            BattleCommand::Patrol { actor } => {
                self.patrol(actor);
            }
        }
    }

    fn move_toward(&mut self, actor: Entity, destination: f64) {
        let Some((pos, speed, alive)) = self
            .combatant(actor)
            .map(|combatant| (combatant.pos, combatant.speed, combatant.alive))
        else {
            return;
        };
        if !alive {
            return;
        }

        let step = speed * self.dt.as_secs_f64();
        let delta = destination - pos;
        if delta.abs() <= f64::EPSILON {
            return;
        }
        let new_pos = if delta.abs() <= step {
            destination
        } else {
            pos + delta.signum() * step
        };

        if let Some(combatant) = self.combatant_mut(actor) {
            combatant.pos = new_pos;
        }
    }

    fn move_away(&mut self, actor: Entity, from: f64) {
        let Some((pos, speed, alive)) = self
            .combatant(actor)
            .map(|combatant| (combatant.pos, combatant.speed, combatant.alive))
        else {
            return;
        };
        if !alive {
            return;
        }

        let step = speed * self.dt.as_secs_f64();
        let direction = if pos < from {
            -1.0
        } else if pos > from {
            1.0
        } else {
            -1.0
        };

        if let Some(combatant) = self.combatant_mut(actor) {
            combatant.pos += direction * step;
        }
    }

    fn strike(&mut self, actor: Entity, target: Entity, skill: bool) {
        let Some((range, damage, actor_pos, actor_name, actor_alive)) =
            self.combatant(actor).map(|combatant| {
                (
                    combatant.range,
                    if skill { 28.0 } else { combatant.damage },
                    combatant.pos,
                    combatant.name.clone(),
                    combatant.alive,
                )
            })
        else {
            return;
        };
        if !actor_alive {
            return;
        }

        let Some((target_pos, target_name, target_alive)) = self
            .combatant(target)
            .map(|combatant| (combatant.pos, combatant.name.clone(), combatant.alive))
        else {
            return;
        };
        if !target_alive || (actor_pos - target_pos).abs() > range {
            return;
        }

        let killed = {
            let Some(target_mut) = self.combatant_mut(target) else {
                return;
            };
            target_mut.hp -= damage;
            if target_mut.hp <= 0.0 {
                target_mut.hp = 0.0;
                target_mut.alive = false;
                true
            } else {
                false
            }
        };

        if killed {
            self.log
                .push(format!("{actor_name} knocks out {target_name}"));
        } else {
            let verb = if skill { "skill-hits" } else { "attacks" };
            self.log
                .push(format!("{actor_name} {verb} {target_name} (-{damage:.0})"));
        }
    }

    fn drink(&mut self, actor: Entity) {
        let Some((potions, hp, max_hp, heal, alive, name)) =
            self.combatant(actor).map(|combatant| {
                (
                    combatant.potions,
                    combatant.hp,
                    combatant.max_hp,
                    combatant.potion_heal,
                    combatant.alive,
                    combatant.name.clone(),
                )
            })
        else {
            return;
        };
        if !alive || potions == 0 || hp >= max_hp {
            return;
        }

        if let Some(combatant) = self.combatant_mut(actor) {
            combatant.hp = (combatant.hp + heal).min(combatant.max_hp);
            combatant.potions -= 1;
        }
        self.log.push(format!(
            "{name} drinks a potion (+{heal:.0}), {} left",
            potions - 1
        ));
    }

    fn patrol(&mut self, actor: Entity) {
        if actor != Entity::Player || !self.player.alive {
            return;
        }
        if self.tick.is_multiple_of(4) {
            self.log.push(String::from("player patrols"));
        }
        self.player.pos += 0.1;
    }
}

// ==================== Blackboard keys and command helpers ====================

static COMMANDS: Key<CommandBuffer<BattleCommand>> = playact::key!("commands");
static TARGET: Key<Option<usize>> = playact::key!("target");

fn push_command(ctx: &mut Context<'_, World>, command: BattleCommand) {
    ctx.blackboard()
        .get_or_insert_with(&COMMANDS, CommandBuffer::new)
        .push(command);
}

fn target_index(ctx: &Context<'_, World>) -> Option<usize> {
    ctx.blackboard_ref().get(&TARGET).copied().flatten()
}

fn target<'a>(ctx: &'a Context<'_, World>) -> Option<&'a Combatant> {
    let index = target_index(ctx)?;
    ctx.world_ref()
        .goblins
        .get(index)
        .filter(|goblin| goblin.alive)
}

// ==================== Player conditions ====================

fn hp_below(ctx: &Context<'_, World>, ratio: f64) -> bool {
    let player = &ctx.world_ref().player;
    player.alive && player.hp_ratio() < ratio
}

fn has_potion(ctx: &Context<'_, World>) -> bool {
    let player = &ctx.world_ref().player;
    player.alive && player.potions > 0
}

fn has_living_target(ctx: &Context<'_, World>) -> bool {
    let world = ctx.world_ref();
    world.player.alive && world.nearest_goblin().is_some()
}

fn target_in_range(ctx: &Context<'_, World>) -> bool {
    let Some(target) = target(ctx) else {
        return false;
    };
    let player = &ctx.world_ref().player;
    player.alive && (target.pos - player.pos).abs() <= player.range
}

fn target_low_hp(ctx: &Context<'_, World>) -> bool {
    target(ctx).is_some_and(|target| target.hp_ratio() < 0.25)
}

// ==================== Player actions ====================

fn select_target(ctx: &mut Context<'_, World>) -> Status {
    let nearest = ctx.world_ref().nearest_goblin().map(|(index, _)| index);
    ctx.blackboard().set(&TARGET, nearest);
    if nearest.is_some() {
        Status::Success
    } else {
        Status::Failure
    }
}

fn drink_potion(ctx: &mut Context<'_, World>) -> Status {
    push_command(
        ctx,
        BattleCommand::DrinkPotion {
            actor: Entity::Player,
        },
    );
    Status::Success
}

fn retreat(ctx: &mut Context<'_, World>) -> Status {
    let Some((_, distance)) = ctx.world_ref().nearest_goblin() else {
        return Status::Success;
    };
    let from = match ctx.world_ref().nearest_goblin() {
        Some((index, _)) => ctx.world_ref().goblins[index].pos,
        None => return Status::Success,
    };

    push_command(
        ctx,
        BattleCommand::MoveAway {
            actor: Entity::Player,
            from,
        },
    );

    if distance < 8.0 {
        Status::Running
    } else {
        Status::Success
    }
}

fn cast_skill(ctx: &mut Context<'_, World>) -> Status {
    let Some(index) = target_index(ctx) else {
        return Status::Failure;
    };
    push_command(
        ctx,
        BattleCommand::CastSkill {
            actor: Entity::Player,
            target: Entity::Goblin(index),
        },
    );
    Status::Success
}

fn attack(ctx: &mut Context<'_, World>) -> Status {
    let Some(index) = target_index(ctx) else {
        return Status::Failure;
    };
    push_command(
        ctx,
        BattleCommand::Attack {
            actor: Entity::Player,
            target: Entity::Goblin(index),
        },
    );
    Status::Success
}

fn approach(ctx: &mut Context<'_, World>) -> Status {
    let Some(target) = target(ctx) else {
        return Status::Failure;
    };
    push_command(
        ctx,
        BattleCommand::MoveTo {
            actor: Entity::Player,
            destination: target.pos,
        },
    );
    Status::Running
}

fn hold(_ctx: &mut Context<'_, World>) -> Status {
    Status::Running
}

fn patrol(ctx: &mut Context<'_, World>) -> Status {
    push_command(
        ctx,
        BattleCommand::Patrol {
            actor: Entity::Player,
        },
    );
    Status::Success
}

// ==================== Goblin conditions/actions ====================

fn goblin_in_range(ctx: &Context<'_, World>, index: usize) -> bool {
    let world = ctx.world_ref();
    let Some(goblin) = world.goblins.get(index) else {
        return false;
    };
    goblin.alive && world.player.alive && (goblin.pos - world.player.pos).abs() <= goblin.range
}

fn goblin_attack(ctx: &mut Context<'_, World>, index: usize) -> Status {
    push_command(
        ctx,
        BattleCommand::Attack {
            actor: Entity::Goblin(index),
            target: Entity::Player,
        },
    );
    Status::Success
}

fn goblin_chase(ctx: &mut Context<'_, World>, index: usize) -> Status {
    let world = ctx.world_ref();
    if !world.player.alive {
        return Status::Failure;
    }
    let destination = world.player.pos;
    push_command(
        ctx,
        BattleCommand::MoveTo {
            actor: Entity::Goblin(index),
            destination,
        },
    );
    Status::Running
}

fn goblin_hold(_ctx: &mut Context<'_, World>) -> Status {
    Status::Running
}

// ==================== Tree factories ====================

fn build_player_tree() -> BehaviorTree<World> {
    BehaviorTreeBuilder::<World>::new("player")
        .reactive_selector(|b| {
            // P1 survival: drink a potion or retreat at low HP, preempting combat at any time.
            b.sequence(|b| {
                b.condition("low hp", |ctx| hp_below(ctx, 0.30))
                    .reactive_selector(|b| {
                        b.cooldown(Duration::from_secs(5), |b| {
                            b.sequence(|b| {
                                b.condition("has potion", has_potion)
                                    .action("drink potion", drink_potion)
                            })
                        })
                        .action("retreat", retreat)
                    })
            })
            // P2 combat: target selection is refreshed every tick.
            .reactive_sequence(|b| {
                b.condition("has living target", has_living_target)
                    .action("select target", select_target)
                    .reactive_selector(|b| {
                        // Finisher: skill takes priority when the target is low and in range.
                        b.sequence(|b| {
                            b.condition("target low hp", target_low_hp)
                                .condition("target in range", target_in_range)
                                .cooldown(Duration::from_secs(4), |b| {
                                    b.action("cast skill", cast_skill)
                                })
                        })
                        // Basic attack: in range and off cooldown.
                        .sequence(|b| {
                            b.condition("target in range", target_in_range)
                                .cooldown(Duration::from_millis(750), |b| {
                                    b.action("basic attack", attack)
                                })
                        })
                        // Approach: move while out of range; Timeout prevents an endless chase.
                        .sequence(|b| {
                            b.inverter(|b| b.condition("target in range", target_in_range))
                                .timeout(Duration::from_secs(5), |b| {
                                    b.action("approach target", approach)
                                })
                        })
                        // Hold position on cooldown without preempting higher-priority branches.
                        .action("hold position", hold)
                    })
            })
            // P3 patrol when there is no target.
            .action("patrol", patrol)
        })
        .build()
        .expect("player tree")
}

fn build_goblin_tree(index: usize) -> BehaviorTree<World> {
    BehaviorTreeBuilder::<World>::new(format!("goblin-{index}"))
        .reactive_selector(move |b| {
            b.sequence(move |b| {
                b.condition("player in range", move |ctx| goblin_in_range(ctx, index))
                    .cooldown(Duration::from_millis(1250), move |b| {
                        b.action("attack player", move |ctx| goblin_attack(ctx, index))
                    })
            })
            .sequence(move |b| {
                b.inverter(move |b| {
                    b.condition("player in range", move |ctx| goblin_in_range(ctx, index))
                })
                .timeout(Duration::from_secs(8), move |b| {
                    b.action("chase", move |ctx| goblin_chase(ctx, index))
                })
            })
            .action("hold", goblin_hold)
        })
        .build()
        .expect("goblin tree")
}

// ==================== Tracer ====================

#[derive(Clone, Default)]
struct TraceSink {
    lines: Arc<Mutex<Vec<String>>>,
    aborts: Arc<AtomicUsize>,
}

impl TraceSink {
    fn take(&self) -> Vec<String> {
        let mut lines = self.lines.lock().expect("trace lock");
        lines.drain(..).collect()
    }

    fn abort_count(&self) -> usize {
        self.aborts.load(Ordering::Relaxed)
    }
}

impl Tracer for TraceSink {
    fn event(&mut self, event: playact::TraceEvent<'_>) {
        let line = match event {
            playact::TraceEvent::Abort {
                name,
                kind,
                path,
                reason,
                ..
            } => {
                self.aborts.fetch_add(1, Ordering::Relaxed);
                Some(format!("abort {kind}:{name} path={path:?} reason={reason}"))
            }
            playact::TraceEvent::Exit {
                name, kind, status, ..
            } if kind == Kind::Action && !status.is_running() => {
                Some(format!("{name} -> {status}"))
            }
            _ => None,
        };

        if let Some(line) = line {
            self.lines.lock().expect("trace lock").push(line);
        }
    }
}

// ==================== Driver ====================

fn init_tree(mut tree: BehaviorTree<World>) -> BehaviorTree<World> {
    tree.blackboard_mut().set(&COMMANDS, CommandBuffer::new());
    tree
}

fn drain_commands(tree: &mut BehaviorTree<World>) -> Vec<BattleCommand> {
    match tree.blackboard_mut().get_mut(&COMMANDS) {
        Some(buffer) => buffer.drain().collect(),
        None => Vec::new(),
    }
}

fn main() {
    let mut world = World::new();

    let trace = TraceSink::default();
    let mut player_tree = init_tree(build_player_tree());
    player_tree.set_tracer(Some(Box::new(trace.clone())));

    let mut trees = vec![player_tree];
    for index in 0..world.goblins.len() {
        trees.push(init_tree(build_goblin_tree(index)));
    }

    println!("Player behavior tree:\n{}\n", trees[0].describe());

    for tick in 0..48_u64 {
        world.tick = tick;
        let time = Time::from_millis(tick * 250, 250);

        let mut commands = Vec::new();
        let mut player_status = Status::Running;

        for (index, tree) in trees.iter_mut().enumerate() {
            let status = tree.tick(&mut world, time);
            if index == 0 {
                player_status = status;
            }
            commands.extend(drain_commands(tree));
        }

        let command_count = commands.len();
        for command in commands {
            world.apply(command);
        }

        let trace_lines = trace.take();
        print_tick(tick, &world, player_status, command_count, &trace_lines);
        world.log.clear();

        if !world.player.alive || world.goblins.iter().all(|goblin| !goblin.alive) {
            break;
        }
    }

    let alive_goblins = world.goblins.iter().filter(|goblin| goblin.alive).count();

    println!("=== Result ===");
    println!(
        "player: alive={} hp={:.1} pos={:.1}",
        world.player.alive, world.player.hp, world.player.pos
    );
    println!("goblins alive: {alive_goblins}");
    println!("aborted nodes observed: {}", trace.abort_count());
}

fn print_tick(
    tick: u64,
    world: &World,
    player_status: Status,
    command_count: usize,
    trace: &[String],
) {
    println!(
        "── tick {tick:02}  t={:.2}s  player={player_status}  commands={command_count} ──",
        tick as f64 * 0.25
    );
    println!(
        "  player hp {:5.1}/100  pos {:6.1}  potions {}",
        world.player.hp, world.player.pos, world.player.potions
    );
    for (index, goblin) in world.goblins.iter().enumerate() {
        println!(
            "  goblin#{index} hp {:4.1}/40  pos {:6.1}  {}",
            goblin.hp,
            goblin.pos,
            if goblin.alive { "alive" } else { "down" }
        );
    }
    for line in &world.log {
        println!("  · {line}");
    }
    for line in trace {
        println!("  trace: {line}");
    }
    println!();
}
