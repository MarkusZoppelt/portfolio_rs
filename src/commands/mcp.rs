//! Experimental MCP (Model Context Protocol) server preview.
//!
//! Speaks JSON-RPC over stdio and implements the `initialize` handshake so
//! real MCP clients (Claude Desktop, opencode, ...) can negotiate a session.
//! Tools are advertised via `tools/list`, but the tool methods return
//! pointers to the equivalent CLI commands rather than live data —
//! `tools/call` with real data is planned as a follow-up.

use std::io::{self, BufRead, Write};

pub fn handle_mcp() {
    // Diagnostics go to stderr: stdout is reserved for the JSON-RPC stream.
    eprintln!("portfolio_rs MCP server (experimental preview) listening on stdio...");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        match line {
            Ok(request) => {
                if request.trim().is_empty() {
                    continue;
                }

                // JSON-RPC notifications (no `id`) never get a response,
                // e.g. the `notifications/initialized` sent after `initialize`.
                let Some(response) = process_request(&request) else {
                    continue;
                };
                let Ok(response_str) = serde_json::to_string(&response) else {
                    continue;
                };
                // A failed write means the client closed the pipe; exit quietly.
                if writeln!(stdout, "{}", response_str).is_err() || stdout.flush().is_err() {
                    break;
                }
            }
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct JsonRpcRequest {
    id: Option<serde_json::Value>,
    method: String,
}

#[derive(serde::Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(serde::Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

fn ok_response(id: Option<serde_json::Value>, result: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(result),
        error: None,
    }
}

fn error_response(id: Option<serde_json::Value>, code: i32, message: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(JsonRpcError { code, message }),
    }
}

/// Handle one JSON-RPC message. Returns `None` for notifications (messages
/// with no `id`), which per the JSON-RPC spec never get a response — e.g.
/// the `notifications/initialized` a client sends right after `initialize`.
fn process_request(request_str: &str) -> Option<JsonRpcResponse> {
    let request: JsonRpcRequest = match serde_json::from_str(request_str) {
        Ok(req) => req,
        Err(e) => return Some(error_response(None, -32700, format!("Parse error: {}", e))),
    };

    let id = request.id.clone();
    id.as_ref()?; // notification: no `id`, no response expected

    let response = match request.method.as_str() {
        "initialize" => ok_response(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "portfolio_rs",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "get_portfolio_snapshot" => ok_response(
            id,
            serde_json::json!({
                "status": "ok",
                "note": "Preview stub. Run: portfolio_rs context <FILE> --format json"
            }),
        ),
        "get_allocation" => ok_response(
            id,
            serde_json::json!({
                "status": "ok",
                "note": "Preview stub. Run: portfolio_rs allocation <FILE>"
            }),
        ),
        "get_context_markdown" => ok_response(
            id,
            serde_json::json!({
                "status": "ok",
                "note": "Preview stub. Run: portfolio_rs context <FILE> --format markdown"
            }),
        ),
        "get_investment_policy" => ok_response(
            id,
            serde_json::json!({
                "status": "ok",
                "note": "Preview stub. Read portfolio/policy.toml or INVESTMENT_POLICY.md"
            }),
        ),
        "tools/list" => ok_response(
            id,
            serde_json::json!({
                "tools": [
                    { "name": "get_portfolio_snapshot", "description": "Get portfolio snapshot" },
                    { "name": "get_allocation", "description": "Get asset allocation" },
                    { "name": "get_context_markdown", "description": "Get context briefing" },
                    { "name": "get_investment_policy", "description": "Get investment policy" }
                ]
            }),
        ),
        method => error_response(id, -32601, format!("Method not found: {}", method)),
    };

    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_handshake() {
        let response =
            process_request(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).unwrap();
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert!(result.get("protocolVersion").is_some());
        assert_eq!(result["serverInfo"]["name"], "portfolio_rs");
    }

    #[test]
    fn test_notification_gets_no_response() {
        // No `id`: a JSON-RPC notification, e.g. what a client sends after
        // `initialize` (`notifications/initialized`). Must not respond.
        let response = process_request(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(response.is_none());
    }

    #[test]
    fn test_tools_list() {
        let response =
            process_request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert_eq!(result["tools"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn test_unknown_method() {
        let response = process_request(r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#).unwrap();
        assert_eq!(response.error.unwrap().code, -32601);
    }

    #[test]
    fn test_parse_error() {
        let response = process_request("not json").unwrap();
        assert_eq!(response.error.unwrap().code, -32700);
    }
}
