use crate::script_lib::BevyScriptLib;
use crate::script_lib_handle::LuaScriptLibHandle;
use bevy_ecs::prelude::*;
use mlua::Lua;
use snezhnaya_gi_data::scene::script_cache::{compiled_lua_scripts, create_lua_vm};
use std::sync::Arc;

#[derive(Resource, Clone)]
pub struct LuaRuntime {
    pub lua: Lua,
}

impl LuaRuntime {
    pub fn new(script_lib: Arc<BevyScriptLib>, protocol_version: String) -> Self {
        let lua = get_lua(script_lib, protocol_version);

        Self { lua }
    }
}

pub fn get_lua(script_lib: Arc<BevyScriptLib>, protocol_version: String) -> Lua {
    // 每个世界独立保存 ScriptLib，避免玩家之间覆盖上下文。
    let lua = create_lua_vm("assets/lua/common");

    let globals = lua.globals();

    let lib_handle = LuaScriptLibHandle {
        script_lib,
        protocol_version,
    };
    globals
        .set("ScriptLib", lib_handle)
        .expect("binding world ScriptLib");

    let scripts = compiled_lua_scripts("assets/lua/gadget").expect("loading gadget Lua scripts");
    for script in scripts.iter() {
        let script_name = format!("{}.lua", script.name);
        let env = lua.create_table().unwrap();
        let mt = lua.create_table().unwrap();
        mt.set("__index", globals.clone()).unwrap();
        env.set_metatable(Some(mt)).unwrap();
        if let Err(error) = lua
            .load(script.bytecode.as_ref())
            .set_environment(env.clone())
            .exec()
        {
            tracing::warn!(%error, script = %script_name, "gadget Lua could not be loaded");
            continue;
        }
        globals.set(script_name, env).unwrap();
    }

    lua
}
