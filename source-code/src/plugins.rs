use anyhow::Result;
use mlua::{Lua, Function, LuaSerdeExt, RegistryKey};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

// We wrap Lua in a Mutex because we will share PluginManager across async tasks (Arc)
pub struct PluginManager {
    lua: Mutex<Lua>,
    hooks: Mutex<HashMap<String, Vec<RegistryKey>>>,
}

impl PluginManager {
    pub fn new() -> Result<Self> {
        let lua = Lua::new();

        {
            // Expose LPM API to Lua
            // Scoped block ensures 'globals' is dropped before we move 'lua' into Mutex
            let globals = lua.globals();
            let lpm = lua.create_table()?;

            // lpm.log("msg")
            lpm.set("log", lua.create_function(|_, msg: String| {
                println!("\x1b[35m[LUA PLUGIN]\x1b[0m {}", msg);
                Ok(())
            })?)?;

            globals.set("lpm", lpm)?;
        }

        Ok(Self {
            lua: Mutex::new(lua),
           hooks: Mutex::new(HashMap::new()),
        })
    }

    pub fn load_all(&self) -> Result<()> {
        let plugin_dir = Path::new("/etc/lpm/plugins");
        if !plugin_dir.exists() { return Ok(()); }

        let lua = self.lua.lock().unwrap();

        lua.load(r#"
        lpm._hooks = {}
        function lpm.register_hook(event, callback)
        table.insert(lpm._hooks, {event=event, callback=callback})
        end
        "#).exec()?;

        // Load files
        for entry in fs::read_dir(plugin_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "lua") {
                let code = fs::read_to_string(&path)?;
                let name = path.file_stem().unwrap().to_string_lossy();
                lua.load(&code).set_name(name).exec()?;
            }
        }

        // Extract hooks from Lua to Rust Registry
        let globals = lua.globals();
        let lpm: mlua::Table = globals.get("lpm")?;
        let hook_table: mlua::Table = lpm.get("_hooks")?;

        let mut rust_hooks = self.hooks.lock().unwrap();

        for pair in hook_table.sequence_values::<mlua::Table>() {
            let pair = pair?;
            let event: String = pair.get("event")?;
            let callback: Function = pair.get("callback")?;

            let key = lua.create_registry_value(callback)?;
            rust_hooks.entry(event).or_default().push(key);
        }

        Ok(())
    }

    pub fn run_hook(&self, hook_name: &str, payload: impl serde::Serialize) -> Result<()> {
        let lua = self.lua.lock().unwrap();
        let hooks_lock = self.hooks.lock().unwrap();

        if let Some(keys) = hooks_lock.get(hook_name) {
            let lua_payload = lua.to_value(&payload)?;
            for key in keys {
                let func: Function = lua.registry_value(key)?;
                if let Err(e) = func.call::<_, ()>(lua_payload.clone()) {
                    eprintln!("Plugin Hook Error ({}): {}", hook_name, e);
                    // For security hooks (pre_install), maybe we should panic or return Err?
                    if hook_name == "on_pre_install" {
                        return Err(anyhow::anyhow!("Plugin blocked installation: {}", e));
                    }
                }
            }
        }
        Ok(())
    }
}
