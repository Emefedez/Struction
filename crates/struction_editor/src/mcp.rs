//! A Model Context Protocol server over stdio: newline-delimited JSON-RPC 2.0 offering the
//! [`ai`] tools, so MCP clients (Claude Code, Claude Desktop, IDE agents) author a project
//! through the same operations as the editor. Like [`protocol::serve`](crate::protocol::serve)
//! it is synchronous and the host must keep logs off stdout.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use crate::AuthoringProject;
use crate::ai;

/// Newest first; a client asking for another gets the newest.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub fn serve(
    project: &mut AuthoringProject,
    input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(project, message),
            Err(error) => Some(failure(Value::Null, -32700, &error.to_string())),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// The response to one message; notifications and client responses get none.
pub fn handle(project: &mut AuthoringProject, message: Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return id
            .filter(|_| message.get("result").is_none() && message.get("error").is_none())
            .map(|id| failure(id, -32600, "Expected a JSON-RPC request"));
    };
    let id = id?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = PROTOCOL_VERSIONS
                .into_iter()
                .find(|version| Some(*version) == asked)
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            success(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "struction", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": ai::INSTRUCTIONS,
                }),
            )
        }
        "ping" => success(id, json!({})),
        "tools/list" => {
            let tools: Vec<_> = ai::tools()
                .into_iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    })
                })
                .collect();
            success(id, json!({ "tools": tools }))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(failure(id, -32602, "tools/call needs a tool `name`"));
            };
            if !ai::tools().iter().any(|tool| tool.name == name) {
                return Some(failure(id, -32602, &format!("Unknown tool `{name}`")));
            }
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let (result, is_error) = match ai::call(project, name, arguments) {
                Ok(result) => (result, false),
                Err(failure) => (json!(failure), true),
            };
            success(
                id,
                json!({
                    "content": [{ "type": "text", "text": result.to_string() }],
                    "isError": is_error,
                }),
            )
        }
        other => failure(id, -32601, &format!("Method `{other}` is not supported")),
    })
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
