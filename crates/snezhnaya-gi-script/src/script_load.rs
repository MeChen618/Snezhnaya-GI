use crate::script_lib::{GroupVariableStore, call_lua_trigger_action, call_lua_trigger_condition};
use mlua::{Function, Lua};
use snezhnaya_gi_data::scene::scene_group_template::SceneGroupTemplate;
use snezhnaya_gi_data::scene::script_cache::{load_scene_group_from_cache, scene_group_is_bad};
use snezhnaya_gi_data::scene::{EventType, LuaContext, LuaEvt};
use std::collections::HashMap;
use std::sync::atomic::Ordering;

pub struct CompiledTrigger {
    pub name: String,
    pub source: String,
    pub limit: i32,
    pub count: u32,
    pub disabled: bool,
    pub condition: Option<Function>,
    pub action: Option<Function>,
}

pub enum GroupLoadState {
    Unloaded,
    Loading,
    Loaded(SceneGroupRuntime),
    Failed,
}

pub struct SceneGroupRuntime {
    pub data: SceneGroupTemplate,
    pub context: LuaContext,
    pub triggers_by_event: HashMap<EventType, Vec<CompiledTrigger>>,
    pub active_suites: Vec<u32>,
    pub active_trigger_names: Vec<String>,
    pub variables: HashMap<String, i32>,
}

impl SceneGroupRuntime {
    pub fn new(
        scene_id: u32,
        block_id: u32,
        group_id: u32,
        player_uid: u32,
        lua: &Lua,
    ) -> Option<Self> {
        if scene_group_is_bad(group_id) {
            return None;
        }

        let globals = lua.globals();

        let lua_root = "./assets/lua";

        let script_name = format!("scene{}_group{}", scene_id, group_id);
        let script_path = format!("{}/scene/{}/{}.lua", lua_root, scene_id, script_name);

        let code = common::string_util::read_utf8_no_bom(&script_path).ok()?;
        let code = code.replace("ScriptLib.", "ScriptLib:");

        let env = lua.create_table().ok()?;
        let mt = lua.create_table().ok()?;
        mt.set("__index", globals.clone()).ok()?;
        env.set_metatable(Some(mt)).ok()?;

        let chunk = lua.load(&code).set_name(&script_name);
        chunk.set_environment(env.clone()).exec().ok()?;

        globals.set(script_name.clone(), env.clone()).ok()?;

        let Some(data) = load_scene_group_from_cache(lua_root, &lua, scene_id, block_id, group_id)
        else {
            let _ = globals.raw_remove(script_name.as_str());
            return None;
        };
        let initial_suite = data.init_config.suite;
        if initial_suite == 0 || initial_suite as usize > data.suites.len() {
            let _ = globals.raw_remove(script_name.as_str());
            return None;
        }

        if data.triggers.is_empty() {
            let variables = data
                .variables
                .iter()
                .map(|v| (v.name.clone(), v.value))
                .collect();

            return Some(Self {
                data,
                context: LuaContext {
                    scene_id,
                    group_id,
                    config_id: 0,
                    source_entity_id: 0,
                    target_entity_id: 0,
                    uid: player_uid,
                },
                triggers_by_event: HashMap::new(),
                active_suites: vec![initial_suite],
                active_trigger_names: Vec::new(),
                variables,
            });
        }

        let mut triggers_by_event: HashMap<EventType, Vec<CompiledTrigger>> = HashMap::new();

        for trig in &data.triggers {
            let cond = if trig.condition.is_empty() {
                None
            } else {
                match env.get::<Function>(trig.condition.as_str()) {
                    Ok(condition) => Some(condition),
                    Err(error) => {
                        tracing::warn!(group_id, trigger = %trig.name, %error, "Lua condition is missing");
                        continue;
                    }
                }
            };

            let action = (!trig.action.is_empty())
                .then_some(trig.action.as_str())
                .and_then(|name| env.get::<Function>(name).ok());

            if action.is_some() || trig.action.is_empty() {
                triggers_by_event
                    .entry(trig.event.clone())
                    .or_default()
                    .push(CompiledTrigger {
                        name: trig.name.clone(),
                        source: trig.source.clone(),
                        limit: trig.trigger_count.unwrap_or(1),
                        count: 0,
                        disabled: false,
                        condition: cond,
                        action,
                    });
            }
        }

        if triggers_by_event.is_empty() {
            tracing::debug!("SceneGroupRuntime {} has no valid triggers", group_id);
        }

        let variables = data
            .variables
            .iter()
            .map(|v| (v.name.clone(), v.value))
            .collect();

        let mut rt = Self {
            data,
            context: LuaContext {
                scene_id,
                group_id,
                config_id: 0,
                source_entity_id: 0,
                target_entity_id: 0,
                uid: player_uid,
            },
            triggers_by_event,
            active_suites: vec![initial_suite],
            active_trigger_names: Vec::new(),
            variables,
        };

        rt.recompute_active_triggers();
        Some(rt)
    }

    pub fn recompute_active_triggers(&mut self) {
        let mut names = Vec::new();

        for suite_id in &self.active_suites {
            let idx = (*suite_id as usize).saturating_sub(1);
            if let Some(suite) = self.data.suites.get(idx) {
                for name in &suite.triggers {
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }
        }

        // 小组重新加入后，恢复其中触发器的次数。
        for trigger in self.triggers_by_event.values_mut().flatten() {
            if names.contains(&trigger.name) && !self.active_trigger_names.contains(&trigger.name) {
                trigger.count = 0;
                trigger.disabled = false;
            }
        }
        self.active_trigger_names = names;
    }

    pub fn reset_triggers(&mut self) {
        self.active_trigger_names.clear();
        self.recompute_active_triggers();
    }

    pub fn dispatch(
        &mut self,
        event_type: &EventType,
        event: &LuaEvt,
        variables: &GroupVariableStore,
    ) {
        let Some(triggers) = self.triggers_by_event.get_mut(event_type) else {
            return;
        };
        let mut evt = event.clone();
        evt.event_type = event_type.clone() as u32;
        if evt.uid == 0 {
            evt.uid = self.context.uid;
        }
        if evt.source_name.is_empty() {
            evt.source_name = match event_type {
                EventType::EventEnterRegion | EventType::EventLeaveRegion => "1".to_string(),
                EventType::EventAnyMonsterDie
                | EventType::EventAnyGadgetDie
                | EventType::EventChallengeSuccess
                | EventType::EventChallengeFail => evt.param1.to_string(),
                _ => String::new(),
            };
        }
        let mut ctx = self.context;
        ctx.source_entity_id = evt.source_eid;
        ctx.target_entity_id = evt.target_eid;
        for trigger in triggers {
            if trigger.disabled
                || !self.active_trigger_names.contains(&trigger.name)
                || (!trigger.source.is_empty() && trigger.source != evt.source_name)
            {
                continue;
            }
            if matches!(
                event_type,
                EventType::EventEnterRegion | EventType::EventLeaveRegion
            ) {
                let prefix = if matches!(event_type, EventType::EventEnterRegion) {
                    "ENTER_REGION_"
                } else {
                    "LEAVE_REGION_"
                };
                if let Some(id) = trigger
                    .name
                    .strip_prefix(prefix)
                    .and_then(|id| id.parse::<i64>().ok())
                {
                    if id != evt.param1 {
                        continue;
                    }
                }
            }
            if let Some(condition) = &trigger.condition {
                if !call_lua_trigger_condition(condition, ctx, evt.clone()).unwrap_or(false) {
                    continue;
                }
            }
            let count = trigger.count.saturating_add(1);
            let previous = variables
                .current_trigger_count
                .swap(count, Ordering::Relaxed);
            let success = trigger
                .action
                .as_ref()
                .map(|action| call_lua_trigger_action(action, ctx, evt.clone()).unwrap_or(false))
                .unwrap_or(true);
            variables
                .current_trigger_count
                .store(previous, Ordering::Relaxed);
            trigger.count = count;
            trigger.disabled = !success || (trigger.limit > 0 && count >= trigger.limit as u32);
        }
    }
}
