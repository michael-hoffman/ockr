//! Hello World ockr plugin — demonstrates command registration and HTTP fetch.

use ockr_plugin::{http_get, log, register_command};

ockr_plugin::plugin_metadata! {
    id: "hello-world",
    name: "Hello World",
    version: "0.2.0",
    capabilities: [console, network],
}

#[no_mangle]
pub extern "C" fn ockr_init() {
    register_command("hello-world:greet", "Hello World: Greet", None);
    register_command("hello-world:fetch", "Hello World: Fetch URL", None);
    log("hello-world plugin loaded");
}

#[no_mangle]
pub extern "C" fn ockr_dispatch_command(id_ptr: i32, id_len: i32) {
    let id = unsafe {
        let slice = std::slice::from_raw_parts(id_ptr as *const u8, id_len as usize);
        std::str::from_utf8_unchecked(slice)
    };
    if id == "hello-world:greet" {
        log("Hello from ockr plugin!");
    }
    if id == "hello-world:fetch" {
        // Demonstrate the network capability: fetch a small public JSON endpoint
        // and log the response size so the user can see it in the toast.
        match http_get("https://httpbin.org/get") {
            Some(body) => {
                let msg = format!("fetch ok — {} bytes", body.len());
                log(&msg);
            }
            None => log("fetch failed (no network capability or request error)"),
        }
    }
}
