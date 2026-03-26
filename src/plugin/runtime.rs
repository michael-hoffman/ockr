//! Wasmtime-based plugin runtime.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock, mpsc};

use serde::Deserialize;
use wasmtime::*;
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

use super::panel::{PanelPosition, PluginLayout, RegisteredPanel};

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum PluginEvent {
    CommandRegistered {
        plugin_id: String,
        id: String,
        name: String,
        hint: Option<String>,
    },
    PanelRegistered {
        plugin_id: String,
        panel: RegisteredPanel,
    },
    LogLine {
        plugin_id: String,
        message: String,
    },
    Panicked {
        plugin_id: String,
        message: String,
    },
}

// ── Plugin capabilities (from metadata JSON) ──────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CapabilitiesJson {
    #[serde(default)]
    pub file_read: bool,
    #[serde(default)]
    pub vault_write: bool,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub console: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginMetadataJson {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: CapabilitiesJson,
}

// ── Per-instance store data ───────────────────────────────────────────────────

pub struct PluginData {
    pub wasi: WasiP1Ctx,
    pub plugin_id: String,
    pub event_tx: mpsc::Sender<PluginEvent>,
    /// Shared map written by `ockr_register_package`; keyed by `"@plugin/<name>/..."`.
    pub plugin_packages: Arc<RwLock<HashMap<String, String>>>,
}

// ── Memory helper ─────────────────────────────────────────────────────────────

fn wasm_str(mem: &Memory, store: &impl AsContext, ptr: i32, len: i32) -> Option<String> {
    if ptr < 0 || len < 0 {
        return None;
    }
    mem.data(store)
        .get(ptr as usize..(ptr as usize + len as usize))
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
}

// ── PluginInstance ────────────────────────────────────────────────────────────

pub struct PluginInstance {
    pub plugin_id: String,
    store: Store<PluginData>,
    instance: Instance,
}

impl PluginInstance {
    /// Instantiate with no WASI and a temporary event channel, just to read metadata.
    pub fn probe_metadata(engine: &Engine, wasm: &[u8]) -> Result<PluginMetadataJson, String> {
        let (tx, _rx) = mpsc::channel();
        let tmp = std::env::temp_dir();
        let empty_packages = Arc::new(RwLock::new(HashMap::new()));
        let mut inst = Self::new(
            engine,
            wasm,
            "probe",
            &CapabilitiesJson::default(),
            &tmp,
            tx,
            Arc::clone(&empty_packages),
        )?;
        inst.read_metadata()
    }

    pub fn new(
        engine: &Engine,
        wasm: &[u8],
        plugin_id: &str,
        capabilities: &CapabilitiesJson,
        vault_root: &Path,
        event_tx: mpsc::Sender<PluginEvent>,
        plugin_packages: Arc<RwLock<HashMap<String, String>>>,
    ) -> Result<Self, String> {
        let module = Module::new(engine, wasm).map_err(|e| e.to_string())?;

        // Build WASI context according to capabilities.
        let mut wasi_builder = WasiCtxBuilder::new();
        if capabilities.console {
            wasi_builder.inherit_stdio();
        }
        if capabilities.vault_write {
            wasi_builder
                .preopened_dir(vault_root, "/vault", DirPerms::all(), FilePerms::all())
                .map_err(|e| e.to_string())?;
        } else if capabilities.file_read {
            wasi_builder
                .preopened_dir(vault_root, "/vault", DirPerms::READ, FilePerms::READ)
                .map_err(|e| e.to_string())?;
        }
        let wasi = wasi_builder.build_p1();

        let plugin_data = PluginData {
            wasi,
            plugin_id: plugin_id.to_string(),
            event_tx: event_tx.clone(),
            plugin_packages,
        };
        let mut store = Store::new(engine, plugin_data);

        // Build linker with WASI + host imports.
        let mut linker: Linker<PluginData> = Linker::new(engine);
        preview1::add_to_linker_sync(&mut linker, |data| &mut data.wasi)
            .map_err(|e| e.to_string())?;

        // ockr_register_command(id_p, id_l, nm_p, nm_l, ht_p, ht_l) -> i32
        linker
            .func_wrap(
                "env",
                "ockr_register_command",
                |mut caller: Caller<PluginData>,
                 id_p: i32,
                 id_l: i32,
                 nm_p: i32,
                 nm_l: i32,
                 ht_p: i32,
                 ht_l: i32|
                 -> i32 {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(m)) => m,
                        _ => return -1,
                    };
                    let id = match wasm_str(&mem, &caller, id_p, id_l) {
                        Some(s) => s,
                        None => return -1,
                    };
                    let name = wasm_str(&mem, &caller, nm_p, nm_l).unwrap_or_default();
                    let hint = if ht_l > 0 {
                        wasm_str(&mem, &caller, ht_p, ht_l).filter(|s| !s.is_empty())
                    } else {
                        None
                    };
                    let pid = caller.data().plugin_id.clone();
                    let tx = caller.data().event_tx.clone();
                    let _ = tx.send(PluginEvent::CommandRegistered {
                        plugin_id: pid,
                        id,
                        name,
                        hint,
                    });
                    0
                },
            )
            .map_err(|e| e.to_string())?;

        // ockr_log(ptr, len)
        linker
            .func_wrap(
                "env",
                "ockr_log",
                |mut caller: Caller<PluginData>, ptr: i32, len: i32| {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(m)) => m,
                        _ => return,
                    };
                    let msg = wasm_str(&mem, &caller, ptr, len).unwrap_or_default();
                    let pid = caller.data().plugin_id.clone();
                    let tx = caller.data().event_tx.clone();
                    let _ = tx.send(PluginEvent::LogLine {
                        plugin_id: pid,
                        message: msg,
                    });
                },
            )
            .map_err(|e| e.to_string())?;

        // ockr_register_panel(id_p, id_l, ti_p, ti_l, po_p, po_l, la_p, la_l) -> i32
        linker
            .func_wrap(
                "env",
                "ockr_register_panel",
                |mut caller: Caller<PluginData>,
                 id_p: i32,
                 id_l: i32,
                 ti_p: i32,
                 ti_l: i32,
                 po_p: i32,
                 po_l: i32,
                 la_p: i32,
                 la_l: i32|
                 -> i32 {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(m)) => m,
                        _ => return -1,
                    };
                    let panel_id = match wasm_str(&mem, &caller, id_p, id_l) {
                        Some(s) => s,
                        None => return -1,
                    };
                    let title = wasm_str(&mem, &caller, ti_p, ti_l).unwrap_or_default();
                    let pos_str = wasm_str(&mem, &caller, po_p, po_l).unwrap_or_default();
                    let layout_str = wasm_str(&mem, &caller, la_p, la_l).unwrap_or_default();

                    let position = match pos_str.as_str() {
                        "bottom" => PanelPosition::Bottom,
                        "float" => PanelPosition::Float,
                        _ => PanelPosition::Sidebar,
                    };
                    let layout: PluginLayout =
                        serde_json::from_str(&layout_str).unwrap_or(PluginLayout { items: vec![] });

                    let pid = caller.data().plugin_id.clone();
                    let tx = caller.data().event_tx.clone();
                    let _ = tx.send(PluginEvent::PanelRegistered {
                        plugin_id: pid.clone(),
                        panel: RegisteredPanel {
                            plugin_id: pid,
                            panel_id,
                            title,
                            position,
                            layout,
                        },
                    });
                    0
                },
            )
            .map_err(|e| e.to_string())?;

        // ockr_register_package(name_p, name_l, src_p, src_l) -> i32
        // Registers a `@plugin/<plugin_id>/<name>` typst package.
        linker
            .func_wrap(
                "env",
                "ockr_register_package",
                |mut caller: Caller<PluginData>,
                 name_p: i32,
                 name_l: i32,
                 src_p: i32,
                 src_l: i32|
                 -> i32 {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(m)) => m,
                        _ => return -1,
                    };
                    let name = match wasm_str(&mem, &caller, name_p, name_l) {
                        Some(s) => s,
                        None => return -1,
                    };
                    let source = wasm_str(&mem, &caller, src_p, src_l).unwrap_or_default();
                    let pid = caller.data().plugin_id.clone();
                    let key = format!("@plugin/{}/{}", pid, name);
                    if let Ok(mut guard) = caller.data().plugin_packages.write() {
                        guard.insert(key, source);
                    }
                    0
                },
            )
            .map_err(|e| e.to_string())?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| e.to_string())?;

        Ok(Self {
            plugin_id: plugin_id.to_string(),
            store,
            instance,
        })
    }

    /// Read plugin metadata via `ockr_get_metadata` + `ockr_metadata_buf_ptr`.
    pub fn read_metadata(&mut self) -> Result<PluginMetadataJson, String> {
        // 1. Fill METADATA_BUF and get the byte length.
        let meta_len = self
            .instance
            .get_typed_func::<(), i32>(&mut self.store, "ockr_get_metadata")
            .map_err(|e| e.to_string())?
            .call(&mut self.store, ())
            .map_err(|e| e.to_string())?;
        if meta_len <= 0 {
            return Err("ockr_get_metadata returned empty".into());
        }

        // 2. Get the WASM address of METADATA_BUF.
        let meta_ptr = self
            .instance
            .get_typed_func::<(), i32>(&mut self.store, "ockr_metadata_buf_ptr")
            .map_err(|e| e.to_string())?
            .call(&mut self.store, ())
            .map_err(|e| e.to_string())?;

        // 3. Read bytes from linear memory.
        let mem = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or("no memory export")?;
        let json_bytes = mem
            .data(&self.store)
            .get(meta_ptr as usize..(meta_ptr as usize + meta_len as usize))
            .ok_or("metadata buffer out of bounds")?
            .to_vec();
        let json_str = String::from_utf8(json_bytes).map_err(|e| e.to_string())?;
        serde_json::from_str(&json_str).map_err(|e| e.to_string())
    }

    /// Call `ockr_init` on the plugin.
    pub fn init(&mut self) -> Result<(), String> {
        let func = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, "ockr_init")
            .map_err(|e| e.to_string())?;
        func.call(&mut self.store, ()).map_err(|e| {
            let msg = e.to_string();
            let _ = self.store.data().event_tx.send(PluginEvent::Panicked {
                plugin_id: self.plugin_id.clone(),
                message: msg.clone(),
            });
            msg
        })
    }

    /// Dispatch a command to the plugin.
    pub fn dispatch(&mut self, cmd_id: &str) {
        // Write cmd_id into ALLOC_BUF via ockr_alloc.
        let alloc_res = self
            .instance
            .get_typed_func::<i32, i32>(&mut self.store, "ockr_alloc")
            .and_then(|f| f.call(&mut self.store, cmd_id.len() as i32));

        let ptr = match alloc_res {
            Ok(p) if p >= 0 => p,
            _ => {
                let _ = self.store.data().event_tx.send(PluginEvent::Panicked {
                    plugin_id: self.plugin_id.clone(),
                    message: "ockr_alloc failed".into(),
                });
                return;
            }
        };

        // Write cmd bytes into WASM memory.
        let mem = match self.instance.get_memory(&mut self.store, "memory") {
            Some(m) => m,
            None => return,
        };
        let bytes = cmd_id.as_bytes();
        if let Err(e) = mem.write(&mut self.store, ptr as usize, bytes) {
            let _ = self.store.data().event_tx.send(PluginEvent::Panicked {
                plugin_id: self.plugin_id.clone(),
                message: e.to_string(),
            });
            return;
        }

        let dispatch_res = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&mut self.store, "ockr_dispatch_command")
            .and_then(|f| f.call(&mut self.store, (ptr, bytes.len() as i32)));

        if let Err(e) = dispatch_res {
            let _ = self.store.data().event_tx.send(PluginEvent::Panicked {
                plugin_id: self.plugin_id.clone(),
                message: e.to_string(),
            });
        }
    }
}
