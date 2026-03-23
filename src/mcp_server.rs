//! Model Context Protocol (MCP) server for rust-network-mgr.
//!
//! Implements JSON-RPC 2.0 over stdin/stdout, which is the transport used by
//! Claude Desktop, Claude Code, Cursor, and GitHub Copilot Chat.
//!
//! This binary connects to a running `rust-network-mgr` daemon via its HTTP
//! REST API and exposes the daemon's state and control commands as MCP tools
//! and resources.
//!
//! ## Usage (stdio transport)
//! Add to your MCP client config (e.g. claude_desktop_config.json):
//! ```json
//! {
//!   "mcpServers": {
//!     "network-mgr": {
//!       "command": "/path/to/rust-network-mgr-mcp",
//!       "args": ["--api-url", "http://127.0.0.1:9100"]
//!     }
//!   }
//! }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const MCP_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "rust-network-mgr";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 wire types
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    method: String,
    params: Option<Value>,
    id: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0", result: Some(result), error: None, id }
    }

    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(JsonRpcError { code, message: message.into() }),
            id,
        }
    }

    fn method_not_found(id: Option<Value>, method: &str) -> Self {
        Self::err(id, -32601, format!("Method not found: {}", method))
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "get_status",
                "description": "Get the full status of the network manager daemon: interfaces with their IP addresses and tracked Docker container IPs.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "get_interfaces",
                "description": "List all monitored network interfaces and their current IP addresses.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "get_containers",
                "description": "List all tracked Docker containers and their IP addresses as seen by the network manager.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "reload_config",
                "description": "Tell the running daemon to reload its YAML configuration file and re-apply all nftables rules.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "ping_daemon",
                "description": "Check whether the rust-network-mgr daemon is alive and responding.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Resource definitions
// ---------------------------------------------------------------------------

fn resources_list() -> Value {
    json!({
        "resources": [
            {
                "uri": "network://status",
                "name": "Network Manager Status",
                "description": "Full daemon status including interface IPs and container IPs",
                "mimeType": "application/json"
            },
            {
                "uri": "network://interfaces",
                "name": "Network Interfaces",
                "description": "Current network interface state",
                "mimeType": "application/json"
            },
            {
                "uri": "network://containers",
                "name": "Docker Containers",
                "description": "Tracked Docker container IP addresses",
                "mimeType": "application/json"
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// HTTP helper (blocking, uses reqwest-style ureq or raw std::net for zero deps)
// ---------------------------------------------------------------------------

/// Perform a blocking GET against the daemon REST API.
fn http_get(base_url: &str, path: &str) -> Result<Value, String> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = ureq::get(&url)
        .call()
        .map_err(|e| format!("HTTP GET {}: {}", url, e))?;
    response
        .into_json::<Value>()
        .map_err(|e| format!("JSON parse: {}", e))
}

/// Perform a blocking POST against the daemon REST API.
fn http_post(base_url: &str, path: &str) -> Result<Value, String> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = ureq::post(&url)
        .send_json(json!({}))
        .map_err(|e| format!("HTTP POST {}: {}", url, e))?;
    response
        .into_json::<Value>()
        .map_err(|e| format!("JSON parse: {}", e))
}

// ---------------------------------------------------------------------------
// Tool dispatcher
// ---------------------------------------------------------------------------

fn call_tool(name: &str, api_url: &str) -> Value {
    match name {
        "get_status" => match http_get(api_url, "/status") {
            Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }] }),
            Err(e) => json!({ "content": [{ "type": "text", "text": format!("Error: {}", e) }], "isError": true }),
        },
        "get_interfaces" => match http_get(api_url, "/interfaces") {
            Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }] }),
            Err(e) => json!({ "content": [{ "type": "text", "text": format!("Error: {}", e) }], "isError": true }),
        },
        "get_containers" => match http_get(api_url, "/containers") {
            Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }] }),
            Err(e) => json!({ "content": [{ "type": "text", "text": format!("Error: {}", e) }], "isError": true }),
        },
        "reload_config" => match http_post(api_url, "/reload") {
            Ok(v) => json!({ "content": [{ "type": "text", "text": format!("Reload triggered: {}", v) }] }),
            Err(e) => json!({ "content": [{ "type": "text", "text": format!("Error: {}", e) }], "isError": true }),
        },
        "ping_daemon" => match http_get(api_url, "/health") {
            Ok(v) => json!({ "content": [{ "type": "text", "text": format!("Daemon is alive: {}", v) }] }),
            Err(e) => json!({ "content": [{ "type": "text", "text": format!("Daemon unreachable: {}", e) }], "isError": true }),
        },
        other => json!({
            "content": [{ "type": "text", "text": format!("Unknown tool: {}", other) }],
            "isError": true
        }),
    }
}

// ---------------------------------------------------------------------------
// Resource reader
// ---------------------------------------------------------------------------

fn read_resource(uri: &str, api_url: &str) -> Value {
    let path = match uri {
        "network://status" => "/status",
        "network://interfaces" => "/interfaces",
        "network://containers" => "/containers",
        _ => {
            return json!({ "error": format!("Unknown resource URI: {}", uri) });
        }
    };
    match http_get(api_url, path) {
        Ok(v) => json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": v.to_string()
            }]
        }),
        Err(e) => json!({ "error": e }),
    }
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

fn send_response(resp: &JsonRpcResponse) {
    let line = serde_json::to_string(resp).unwrap_or_default();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(line.as_bytes()).ok();
    out.write_all(b"\n").ok();
    out.flush().ok();
}

fn main() {
    // Simple arg parsing: --api-url <url>
    let args: Vec<String> = std::env::args().collect();
    let api_url = args
        .windows(2)
        .find(|w| w[0] == "--api-url")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "http://127.0.0.1:9100".to_string());

    eprintln!("[rust-network-mgr-mcp] starting, daemon API: {}", api_url);

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::err(None, -32700, format!("Parse error: {}", e));
                send_response(&resp);
                continue;
            }
        };

        let id = req.id.clone();
        let resp = match req.method.as_str() {
            // ---- MCP lifecycle ----
            "initialize" => JsonRpcResponse::ok(
                id,
                json!({
                    "protocolVersion": MCP_VERSION,
                    "capabilities": {
                        "tools": { "listChanged": false },
                        "resources": { "subscribe": false, "listChanged": false }
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION
                    }
                }),
            ),
            "initialized" | "notifications/initialized" => continue,
            "ping" => JsonRpcResponse::ok(id, json!({})),

            // ---- Tools ----
            "tools/list" => JsonRpcResponse::ok(id, tools_list()),
            "tools/call" => {
                let tool_name = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let result = call_tool(&tool_name, &api_url);
                JsonRpcResponse::ok(id, result)
            }

            // ---- Resources ----
            "resources/list" => JsonRpcResponse::ok(id, resources_list()),
            "resources/read" => {
                let uri = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("uri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();
                let result = read_resource(&uri, &api_url);
                JsonRpcResponse::ok(id, result)
            }

            other => JsonRpcResponse::method_not_found(id, other),
        };

        send_response(&resp);
    }
}
