// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `orcastudio --mcp` (ADR-027 M2 amendment, item 3): the stdio relay to the running app's MCP
    // socket. Decided before `run()`, so it opens no window, initialises no GTK, and writes nothing
    // to stdout but relayed bytes.
    if orcastudio_lib::is_mcp_relay(std::env::args_os().nth(1).as_deref()) {
        std::process::exit(orcastudio_lib::mcp_relay());
    }
    orcastudio_lib::run()
}
