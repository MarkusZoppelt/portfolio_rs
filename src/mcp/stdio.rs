//! Stdio framing in front of rmcp's line transport.
//!
//! rmcp silently drops lines that are not JSON. MCP clients expect a JSON-RPC
//! `-32700 Parse error` (with `id: null`) instead, and the server must keep
//! serving afterwards. This module reads stdin line by line, answers
//! unparsable lines itself and forwards everything else to rmcp. All stdout
//! writes go through one writer task so responses never interleave. The
//! diagnostic for a malformed line goes to stderr only.

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// Pipe capacity between the filter tasks and rmcp.
const PIPE: usize = 1 << 16;

/// JSON-RPC parse-error response for an unparsable line.
pub fn parse_error_line(err: &serde_json::Error) -> Vec<u8> {
    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": { "code": -32700, "message": format!("Parse error: {err}") }
    });
    let mut out = serde_json::to_vec(&msg).expect("static JSON serializes");
    out.push(b'\n');
    out
}

/// Required string parameter per method. rmcp cannot route a request that
/// lacks it and would answer -32601 (method not found); clients expect
/// -32602 (invalid params).
const REQUIRED_PARAM: &[(&str, &str)] = &[("tools/call", "name"), ("resources/read", "uri")];

fn error_line(id: serde_json::Value, code: i64, message: String) -> Vec<u8> {
    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    let mut out = serde_json::to_vec(&msg).expect("JSON serializes");
    out.push(b'\n');
    out
}

/// Answer requests rmcp would mishandle, if any:
/// - a batch (JSON array) or any non-object message: `-32600` with `id: null`
///   (MCP does not support JSON-RPC batching);
/// - a request missing a required string parameter, or `tools/call` with
///   non-object `arguments`: `-32602`.
fn invalid_request_reply(line: &str) -> Option<Vec<u8>> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.is_array() {
        return Some(error_line(
            serde_json::Value::Null,
            -32600,
            "Invalid Request: batch requests are not supported".to_string(),
        ));
    }
    if !v.is_object() {
        return Some(error_line(
            serde_json::Value::Null,
            -32600,
            "Invalid Request: expected a JSON-RPC object".to_string(),
        ));
    }
    let id = v.get("id")?.clone();
    let method = v.get("method")?.as_str()?;
    if method == "tools/call" {
        if let Some(args) = v.get("params").and_then(|p| p.get("arguments")) {
            if !args.is_object() && !args.is_null() {
                return Some(error_line(
                    id,
                    -32602,
                    "Invalid params: tools/call \"arguments\" must be an object".to_string(),
                ));
            }
        }
    }
    let (_, param) = REQUIRED_PARAM.iter().find(|(m, _)| *m == method)?;
    if v.get("params")
        .and_then(|p| p.get(*param))
        .is_some_and(|x| x.is_string())
    {
        return None;
    }
    Some(error_line(
        id,
        -32602,
        format!("Invalid params: {method} requires a string \"{param}\""),
    ))
}

/// Classify one incoming line: `Ok(true)` forward to rmcp, `Ok(false)` skip
/// (blank), `Err` answer with -32700.
fn check_line(line: &str) -> Result<bool, serde_json::Error> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }
    serde_json::from_str::<serde::de::IgnoredAny>(trimmed).map(|_| true)
}

/// Wrap `input`/`output` so rmcp sees only parseable lines; returns the
/// (reader, writer) pair to hand to `serve`, plus the stdout writer task,
/// which finishes once rmcp's writer and the input side are both closed.
pub fn filtered<R, W>(
    input: R,
    output: W,
) -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<()>,
)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (rmcp_in, mut feed) = tokio::io::duplex(PIPE);
    let (rmcp_out, drain) = tokio::io::duplex(PIPE);
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Single stdout writer.
    let writer = tokio::spawn(async move {
        let mut output = output;
        while let Some(chunk) = rx.recv().await {
            if output.write_all(&chunk).await.is_err() || output.flush().await.is_err() {
                break;
            }
        }
    });

    // rmcp output -> writer (line by line, so a parse error never splits a reply).
    let tx_out = tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(drain);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match lines.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx_out.send(buf.clone()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // stdin -> rmcp, answering unparsable lines here.
    tokio::spawn(async move {
        let mut lines = BufReader::new(input);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match lines.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&buf);
            match check_line(&text) {
                Ok(false) => {}
                Ok(true) => {
                    if let Some(reply) = invalid_request_reply(&text) {
                        if tx.send(reply).is_err() {
                            break;
                        }
                        continue;
                    }
                    if !buf.ends_with(b"\n") {
                        buf.push(b'\n');
                    }
                    if feed.write_all(&buf).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("portfolio_rs MCP server: ignoring malformed input line ({e})");
                    if tx.send(parse_error_line(&e)).is_err() {
                        break;
                    }
                }
            }
        }
        // EOF on stdin: dropping `feed` closes rmcp's input so it shuts down.
        let _ = feed.shutdown().await;
    });

    (rmcp_in, rmcp_out, writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_lines() {
        assert!(!check_line("   \n").unwrap());
        assert!(check_line("{\"jsonrpc\":\"2.0\"}\n").unwrap());
        assert!(check_line("this is not json\n").is_err());
        assert!(check_line("{\"a\":").is_err());
        let e = check_line("nope").unwrap_err();
        let v: serde_json::Value = serde_json::from_slice(&parse_error_line(&e)).unwrap();
        assert_eq!(v["error"]["code"], -32700);
        assert!(v["id"].is_null());
    }

    #[test]
    fn missing_tool_name_is_invalid_params() {
        let r =
            invalid_request_reply(r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{}}"#)
                .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&r).unwrap();
        assert_eq!(v["error"]["code"], -32602);
        assert_eq!(v["id"], 7);
        assert!(invalid_request_reply(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"x"}}"#
        )
        .is_none());
        assert!(invalid_request_reply(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).is_none());
    }

    /// t8: batches and non-object messages are -32600 with id null;
    /// non-object tool arguments are -32602.
    #[test]
    fn batch_and_bad_arguments_are_rejected() {
        let parse = |r: Vec<u8>| serde_json::from_slice::<serde_json::Value>(&r).unwrap();
        for line in [
            "[1,2]",
            "[]",
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#,
            "42",
        ] {
            let v = parse(invalid_request_reply(line).unwrap());
            assert_eq!(v["error"]["code"], -32600, "{line}");
            assert!(v["id"].is_null(), "{line}");
        }
        for args in ["[1]", "\"x\"", "3"] {
            let line = format!(
                r#"{{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{{"name":"get_positions","arguments":{args}}}}}"#
            );
            let v = parse(invalid_request_reply(&line).unwrap());
            assert_eq!(v["error"]["code"], -32602, "{line}");
            assert_eq!(v["id"], 9);
        }
        assert!(invalid_request_reply(
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"x","arguments":{}}}"#
        )
        .is_none());
    }
}
