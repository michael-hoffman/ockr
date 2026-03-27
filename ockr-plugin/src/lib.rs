//! ockr-plugin SDK — compile with `--target wasm32-wasip1`.
//!
//! Plugins call the safe wrappers (`register_command`, `log`, `register_panel`)
//! from `ockr_init`. The host (Wasmtime) links the `extern "C"` imports at
//! instantiation time.

// ── Host imports ─────────────────────────────────────────────────────────────

extern "C" {
    fn ockr_register_command(
        id_p: i32, id_l: i32,
        nm_p: i32, nm_l: i32,
        ht_p: i32, ht_l: i32,
    ) -> i32;
    fn ockr_log(ptr: i32, len: i32);
    fn ockr_register_panel(
        id_p: i32, id_l: i32,
        ti_p: i32, ti_l: i32,
        po_p: i32, po_l: i32,
        la_p: i32, la_l: i32,
    ) -> i32;
    fn ockr_register_package(name_p: i32, name_l: i32, src_p: i32, src_l: i32) -> i32;
    /// Fetch `url` via HTTP GET. Response body is written into `HTTP_BUF`.
    /// Returns the number of bytes written, or -1 if the request failed or
    /// the plugin does not have the `network` capability.
    fn ockr_http_get(url_p: i32, url_l: i32) -> i32;
}

// ── Safe wrappers ─────────────────────────────────────────────────────────────

/// Register a command that will appear in the ockr command palette.
pub fn register_command(id: &str, name: &str, hint: Option<&str>) {
    let hint_str = hint.unwrap_or("");
    unsafe {
        ockr_register_command(
            id.as_ptr() as i32, id.len() as i32,
            name.as_ptr() as i32, name.len() as i32,
            hint_str.as_ptr() as i32, hint_str.len() as i32,
        );
    }
}

/// Write a log line visible in ockr's notification toast.
pub fn log(msg: &str) {
    unsafe { ockr_log(msg.as_ptr() as i32, msg.len() as i32); }
}

/// Register a typst package accessible as `#import "@plugin/<plugin_id>/<name>"`.
///
/// `name` should be a filename like `"lib.typ"`.
/// `source` is the full typst source text of the package.
pub fn register_typst_package(name: &str, source: &str) {
    unsafe {
        ockr_register_package(
            name.as_ptr() as i32,   name.len() as i32,
            source.as_ptr() as i32, source.len() as i32,
        );
    }
}

/// Register a UI panel. `position` must be `"sidebar"`, `"bottom"`, or `"float"`.
/// `layout_json` is a JSON string matching `PluginLayout` (see host docs).
pub fn register_panel(id: &str, title: &str, position: &str, layout_json: &str) {
    unsafe {
        ockr_register_panel(
            id.as_ptr() as i32,           id.len() as i32,
            title.as_ptr() as i32,        title.len() as i32,
            position.as_ptr() as i32,     position.len() as i32,
            layout_json.as_ptr() as i32,  layout_json.len() as i32,
        );
    }
}

/// Perform an HTTP GET request for `url`.
///
/// Requires the `network` capability. Returns the response body as a `String`
/// on success, or `None` if the request failed or the capability is not granted.
pub fn http_get(url: &str) -> Option<String> {
    let len = unsafe { ockr_http_get(url.as_ptr() as i32, url.len() as i32) };
    if len < 0 {
        return None;
    }
    let bytes = unsafe { &HTTP_BUF[..len as usize] };
    String::from_utf8(bytes.to_vec()).ok()
}

// ── HTTP response buffer ──────────────────────────────────────────────────────
// The host writes HTTP response bytes here; sdk wrapper reads them back.

static mut HTTP_BUF: [u8; 65536] = [0u8; 65536];

/// Returns the pointer to the HTTP response buffer.
/// Called by the host after `ockr_http_get` to locate where it should write.
#[no_mangle]
pub extern "C" fn ockr_http_buf_ptr() -> i32 {
    unsafe { HTTP_BUF.as_ptr() as i32 }
}

// ── Memory allocation export ──────────────────────────────────────────────────
// The host calls ockr_alloc to write strings into WASM memory (e.g. command id
// passed to ockr_dispatch_command).

static mut ALLOC_BUF: [u8; 4096] = [0u8; 4096];

#[no_mangle]
pub extern "C" fn ockr_alloc(len: i32) -> i32 {
    // Simple bump-pointer: always returns the start of ALLOC_BUF.
    // Plugins are single-threaded; the host writes, then immediately calls the
    // export that reads, so this is safe.
    if (len as usize) > unsafe { ALLOC_BUF.len() } {
        return -1;
    }
    unsafe { ALLOC_BUF.as_ptr() as i32 }
}

// ── Metadata buffer (used by plugin_metadata! macro) ─────────────────────────

#[doc(hidden)]
pub static mut METADATA_BUF: [u8; 512] = [0u8; 512];

/// Returns the pointer to METADATA_BUF. Called by the host after
/// `ockr_get_metadata()` to locate the filled buffer.
#[no_mangle]
pub extern "C" fn ockr_metadata_buf_ptr() -> i32 {
    unsafe { METADATA_BUF.as_ptr() as i32 }
}

// ── plugin_metadata! macro ────────────────────────────────────────────────────

/// Generates an `ockr_get_metadata() -> i32` export.
///
/// ```
/// ockr_plugin::plugin_metadata! {
///     id: "my-plugin",
///     name: "My Plugin",
///     version: "0.1.0",
///     capabilities: [console],
/// }
/// ```
///
/// Valid capabilities: `file_read`, `vault_write`, `network`, `console`.
#[macro_export]
macro_rules! plugin_metadata {
    (
        id: $id:literal,
        name: $nm:literal,
        version: $v:literal,
        capabilities: [$($cap:ident),* $(,)?] $(,)?
    ) => {
        #[no_mangle]
        pub extern "C" fn ockr_get_metadata() -> i32 {
            let file_read    = false $( || stringify!($cap) == "file_read"   )*;
            let vault_write  = false $( || stringify!($cap) == "vault_write" )*;
            let network      = false $( || stringify!($cap) == "network"     )*;
            let console      = false $( || stringify!($cap) == "console"     )*;
            let json = ::std::format!(
                r#"{{"id":"{}","name":"{}","version":"{}","capabilities":{{"file_read":{},"vault_write":{},"network":{},"console":{}}}}}"#,
                $id, $nm, $v, file_read, vault_write, network, console
            );
            let bytes = json.as_bytes();
            unsafe {
                let buf = &mut ::ockr_plugin::METADATA_BUF;
                let len = bytes.len().min(buf.len());
                buf[..len].copy_from_slice(&bytes[..len]);
                len as i32
            }
        }
    };
}
