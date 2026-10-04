//! A Model Context Protocol server offering the [`ai`] tools, so any MCP client (Claude Code,
//! Codex, OpenCode, Cursor, IDE agents) authors a project through the same operations as the
//! editor. Two transports: [`serve`], newline-delimited JSON-RPC 2.0 over stdio (synchronous; the
//! host keeps logs off stdout), and [`Endpoint`], Streamable HTTP on localhost, whose requests
//! the host answers on the thread that owns the project, as the editor does every frame.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

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
    respond(Some(project), message)
}

/// Like [`handle`], for a host that may have no project open: tool calls then fail.
fn respond(mut project: Option<&mut AuthoringProject>, message: Value) -> Option<Value> {
    if let Value::Array(batch) = message {
        let responses: Vec<_> = batch
            .into_iter()
            .filter_map(|message| respond(project.as_deref_mut(), message))
            .collect();
        return (!responses.is_empty()).then_some(Value::Array(responses));
    }
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
                        // Edits are validated and undoable, so none destroys anything.
                        "annotations": {
                            "readOnlyHint": tool.read_only,
                            "destructiveHint": false,
                        },
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
            let called = match project {
                Some(project) => ai::call(project, name, arguments).map_err(|f| json!(f)),
                None => Err(json!({ "message": "No project is open in the editor" })),
            };
            let (result, is_error) = match called {
                Ok(result) => (result, false),
                Err(failure) => (failure, true),
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

/// Default port of the editor's endpoint; the next free one is taken when it is in use.
pub const DEFAULT_PORT: u16 = 47_100;

/// A request from an HTTP connection, waiting for the host's answer.
type Pending = (Value, Sender<Option<Value>>);

/// Streamable HTTP on `127.0.0.1` (`POST /mcp`, JSON responses, no server-sent stream). Requests
/// queue until the host calls [`Endpoint::answer`] with its project, so tools run where the
/// project lives. Requests from web pages are refused by their `Origin`.
pub struct Endpoint {
    address: SocketAddr,
    pending: Receiver<Pending>,
}

impl Endpoint {
    /// Listens on the first free port of `port..port + tries`.
    pub fn bind(port: u16, tries: u16) -> io::Result<Self> {
        let mut last = io::Error::other("no port to try");
        for port in (port..=u16::MAX).take(tries.max(1).into()) {
            match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
                Ok(listener) => return Self::listen(listener),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    fn listen(listener: TcpListener) -> io::Result<Self> {
        let address = listener.local_addr()?;
        let (sender, pending) = channel();
        std::thread::Builder::new()
            .name("mcp-endpoint".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let sender = sender.clone();
                    std::thread::spawn(move || {
                        let _ = connection(stream, &sender);
                    });
                }
            })?;
        Ok(Self { address, pending })
    }

    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }

    /// Answers every waiting request; true when a tool call may have changed the project.
    pub fn answer(&self, mut project: Option<&mut AuthoringProject>) -> bool {
        let mut changed = false;
        while let Ok((message, reply)) = self.pending.try_recv() {
            let call = (message["method"] == "tools/call")
                .then(|| message["params"]["name"].as_str().map(str::to_owned))
                .flatten();
            let response = respond(project.as_deref_mut(), message);
            if let Some(name) = call
                && ai::changes(&name)
                && response
                    .as_ref()
                    .is_some_and(|r| r["result"]["isError"] == false)
            {
                changed = true;
            }
            let _ = reply.send(response);
        }
        changed
    }
}

/// How long a request waits for the host, which may be busy (a modal dialog, a long import).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_BODY: usize = 16 << 20;

fn connection(stream: TcpStream, pending: &Sender<Pending>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    // Keep-alive: answer requests until the client closes or goes quiet.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let (mut length, mut origin) = (0, None);
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 {
                return Ok(());
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                match name.trim().to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap_or(0),
                    "origin" => origin = Some(value.trim().to_owned()),
                    _ => {}
                }
            }
        }
        if length > MAX_BODY {
            return reply(&mut writer, "413 Payload Too Large", None);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        let local = origin.as_deref().is_none_or(|origin| {
            let host = origin.split("://").nth(1).unwrap_or(origin);
            let host = host.rsplit_once(':').map_or(host, |(host, _)| host);
            matches!(host, "localhost" | "127.0.0.1" | "[::1]")
        });
        if !local {
            reply(&mut writer, "403 Forbidden", None)?;
            continue;
        }
        if path.split('?').next() != Some("/mcp") {
            reply(&mut writer, "404 Not Found", None)?;
            continue;
        }
        if method != "POST" {
            // No server-initiated stream: the server never sends requests of its own.
            reply(&mut writer, "405 Method Not Allowed", None)?;
            continue;
        }
        let response = match serde_json::from_slice::<Value>(&body) {
            Err(error) => Some(failure(Value::Null, -32700, &error.to_string())),
            Ok(message) => {
                let (sender, answer) = channel();
                if pending.send((message, sender)).is_err() {
                    return reply(&mut writer, "503 Service Unavailable", None);
                }
                match answer.recv_timeout(ANSWER_TIMEOUT) {
                    Ok(response) => response,
                    Err(_) => return reply(&mut writer, "503 Service Unavailable", None),
                }
            }
        };
        match response {
            Some(response) => reply(&mut writer, "200 OK", Some(&response))?,
            None => reply(&mut writer, "202 Accepted", None)?,
        }
    }
}

fn reply(writer: &mut TcpStream, status: &str, body: Option<&Value>) -> io::Result<()> {
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    if status.starts_with("405") {
        head.push_str("Allow: POST\r\n");
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes())?;
    writer.write_all(body.as_bytes())?;
    writer.flush()
}
