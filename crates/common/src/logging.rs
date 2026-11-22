use std::sync::atomic::{AtomicBool, Ordering};
use tracing_subscriber::layer::Layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

pub const TRACE_LOG_PACKET: [&str; 12] = [
    "AbilityInvocationsNotify",
    "ClientAbilityInitFinishNotify",
    "ClientAbilitiesInitFinishCombineNotify",
    "ClientAbilityChangeNotify",
    "CombatInvocationsNotify",
    "PathfindingEnterSceneReq",
    "PathfindingEnterSceneRsp",
    "ToTheMoonEnterSceneReq",
    "ToTheMoonEnterSceneRsp",
    "QueryPathReq",
    "QueryPathRsp",
    "ObstacleModifyNotify",
];

static ABILITY_LOG_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_ability_log(enabled: bool) {
    ABILITY_LOG_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn init() {
    println!("SnezhnayaGI 7.2.0 | based on renyi89/nod-krai-gi");

    let env_filter = EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .without_time()
        .with_target(false);

    let ability_filter = tracing_subscriber::filter::filter_fn(|meta| {
        if meta.target().starts_with("ability") {
            ABILITY_LOG_ENABLED.load(Ordering::Relaxed)
        } else {
            true
        }
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer.with_filter(ability_filter))
        .init();
}
