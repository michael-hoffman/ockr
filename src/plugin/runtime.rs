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
        #[allow(dead_code)] plugin_id: String,
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
    #[allow(dead_code)] pub network: bool,
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
    /// Whether this plugin may make outbound HTTP requests.
    pub network_enabled: bool,
}

// ── Memory helpers ────────────────────────────────────────────────────────────

fn wasm_str(mem: &Memory, store: &impl AsContext, ptr: i32, len: i32) -> Option<String> {
    if ptr < 0 || len < 0 {
        return None;
    }
    mem.data(store)
        .get(ptr as usize..(ptr as usize + len as usize))
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
}

/// Retrieve the `memory` export from a caller, returning `None` if absent.
fn caller_memory(caller: &mut Caller<PluginData>) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Some(m),
        _ => None,
    }
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
            network_enabled: capabilities.network,
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
                 id_p: i32, id_l: i32,
                 nm_p: i32, nm_l: i32,
                 ht_p: i32, ht_l: i32|
                 -> i32 {
                    let Some(mem) = caller_memory(&mut caller) else { return -1; };
                    let Some(id) = wasm_str(&mem, &caller, id_p, id_l) else { return -1; };
                    let name = wasm_str(&mem, &caller, nm_p, nm_l).unwrap_or_default();
                    let hint = if ht_l > 0 {
                        wasm_str(&mem, &caller, ht_p, ht_l).filter(|s| !s.is_empty())
                    } else {
                        None
                    };
                    let data = caller.data();
                    let _ = data.event_tx.send(PluginEvent::CommandRegistered {
                        plugin_id: data.plugin_id.clone(),
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
                    let Some(mem) = caller_memory(&mut caller) else { return; };
                    let msg = wasm_str(&mem, &caller, ptr, len).unwrap_or_default();
                    let data = caller.data();
                    let _ = data.event_tx.send(PluginEvent::LogLine {
                        plugin_id: data.plugin_id.clone(),
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
                 id_p: i32, id_l: i32,
                 ti_p: i32, ti_l: i32,
                 po_p: i32, po_l: i32,
                 la_p: i32, la_l: i32|
                 -> i32 {
                    let Some(mem) = caller_memory(&mut caller) else { return -1; };
                    let Some(panel_id) = wasm_str(&mem, &caller, id_p, id_l) else { return -1; };
                    let title    = wasm_str(&mem, &caller, ti_p, ti_l).unwrap_or_default();
                    let pos_str  = wasm_str(&mem, &caller, po_p, po_l).unwrap_or_default();
                    let lay_str  = wasm_str(&mem, &caller, la_p, la_l).unwrap_or_default();
                    let position = match pos_str.as_str() {
                        "bottom" => PanelPosition::Bottom,
                        "float"  => PanelPosition::Float,
                        _        => PanelPosition::Sidebar,
                    };
                    let layout: PluginLayout =
                        serde_json::from_str(&lay_str).unwrap_or(PluginLayout { items: vec![] });
                    let data = caller.data();
                    let pid  = data.plugin_id.clone();
                    let _ = data.event_tx.send(PluginEvent::PanelRegistered {
                        plugin_id: pid.clone(),
                        panel: RegisteredPanel { plugin_id: pid, panel_id, title, position, layout },
                    });
                    0
                },
            )
            .map_err(|e| e.to_string())?;

        // ockr_register_package(name_p, name_l, src_p, src_l) -> i32
        linker
            .func_wrap(
                "env",
                "ockr_register_package",
                |mut caller: Caller<PluginData>,
                 name_p: i32, name_l: i32,
                 src_p: i32,  src_l: i32|
                 -> i32 {
                    let Some(mem) = caller_memory(&mut caller) else { return -1; };
                    let Some(name) = wasm_str(&mem, &caller, name_p, name_l) else { return -1; };
                    let source = wasm_str(&mem, &caller, src_p, src_l).unwrap_or_default();
                    let data = caller.data();
                    let key  = format!("@plugin/{}/{}", data.plugin_id, name);
                    if let Ok(mut guard) = data.plugin_packages.write() {
                        guard.insert(key, source);
                    }
                    0
                },
            )
            .map_err(|e| e.to_string())?;

        // ockr_http_get(url_p, url_l) -> i32
        // Returns number of bytes written into the plugin's HTTP_BUF, or -1 on error.
        // Only performs actual network I/O if the plugin declared the `network` capability.
        linker
            .func_wrap(
                "env",
                "ockr_http_get",
                |mut caller: Caller<PluginData>, url_p: i32, url_l: i32| -> i32 {
                    if !caller.data().network_enabled {
                        return -1;
                    }
                    // Read URL out of WASM memory (borrow released after block).
                    let url = {
                        let Some(mem) = caller_memory(&mut caller) else { return -1; };
                        match wasm_str(&mem, &caller, url_p, url_l) {
                            Some(u) => u,
                            None => return -1,
                        }
                    };
                    // Blocking HTTP GET — acceptable because plugins run on the thread pool.
                    let body_bytes = match reqwest::blocking::get(&url)
                        .and_then(|r| r.bytes())
                    {
                        Ok(b) => b,
                        Err(_) => return -1,
                    };
                    let write_len = body_bytes.len().min(65535);
                    // Ask the plugin where its HTTP response buffer lives.
                    let buf_ptr: i32 = {
                        let export = caller.get_export("ockr_http_buf_ptr");
                        let func = match export {
                            Some(Extern::Func(f)) => f,
                            _ => return -1,
                        };
                        let typed = match func.typed::<(), i32>(&caller) {
                            Ok(tf) => tf,
                            Err(_) => return -1,
                        };
                        match typed.call(&mut caller, ()) {
                            Ok(p) => p,
                            Err(_) => return -1,
                        }
                    };
                    if buf_ptr < 0 { return -1; }
                    // Write response body into WASM linear memory.
                    let Some(mem) = caller_memory(&mut caller) else { return -1; };
                    if mem.write(&mut caller, buf_ptr as usize, &body_bytes[..write_len]).is_err() {
                        return -1;
                    }
                    write_len as i32
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
