//! Hello World ockr plugin — demonstrates command registration.

use ockr_plugin::{log, register_command};

ockr_plugin::plugin_metadata! {
    id: "hello-world",
    name: "Hello World",
    version: "0.1.0",
    capabilities: [console],
}

#[no_mangle]
pub extern "C" fn ockr_init() {
    register_command("hello-world:greet", "Hello World: Greet", None);
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
}
