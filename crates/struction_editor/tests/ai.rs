//! The AI tool catalog and the MCP server over it.

use std::path::Path;

use bevy::prelude::*;
use serde_json::{Value, json};
use struction_core::*;
use struction_data::DataPlugin;
use struction_editor::{AuthoringProject, ai, mcp, protocol};
use struction_world::*;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Health {
    current: f32,
    max: f32,
}

fn factory(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugins((
        CorePlugin { seed: 7 },
        DataPlugin::new(root).primordial("Actor"),
        WorldPlugin::default(),
    ))
    .register_type::<Health>();
    app
}

const GUARD: &str = "guards/ogre/entity.jsonc";

fn project() -> (tempfile::TempDir, AuthoringProject) {
    let dir = tempfile::tempdir().unwrap();
    for (file, text) in [
        (
            "Actor/entity.jsonc",
            r#"{"components":{"Health":{"current":50,"max":50}}}"#,
        ),
        (
            GUARD,
            "// Ogre guard.\n{\n  \"descendsFrom\": \"Actor\",\n  \"components\": { \"Health\": { \"max\": 60 } }\n}\n",
        ),
        (
            "scenes/courtyard.jsonc",
            r#"{"zones":{"Court":{"position":[10,0,0]}},"spawnerList":{"guards":{"zone":"Court","position":[4,0,6],"spawns":{"ogre":{"definition":"guards/ogre"}}}}}"#,
        ),
    ] {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let project = AuthoringProject::open(dir.path(), factory).unwrap();
    (dir, project)
}

#[test]
fn every_protocol_command_is_a_tool_with_an_object_schema() {
    let (_dir, mut project) = project();
    let describe = serde_json::from_value(json!({ "id": 1, "command": { "op": "describe" } }));
    let response = protocol::execute(&mut project, describe.unwrap());
    let commands = response.result.unwrap()["commands"].clone();
    let tools = ai::tools();
    for command in commands.as_array().unwrap() {
        let command = command.as_str().unwrap();
        // `edit` is offered as `set` and `remove`; MCP has its own discovery.
        if ["edit", "describe"].contains(&command) {
            continue;
        }
        assert!(
            tools.iter().any(|tool| tool.name == command),
            "no tool for `{command}`"
        );
    }
    for tool in &tools {
        assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
        assert!(!tool.description.is_empty());
    }
}

#[test]
fn tools_edit_validate_and_undo_through_the_shared_operations() {
    let (dir, mut project) = project();
    let source = || std::fs::read_to_string(dir.path().join(GUARD)).unwrap();
    let original = source();

    let inspected = ai::call(
        &mut project,
        "inspect_definition",
        json!({ "path": "guards/ogre" }),
    )
    .unwrap();
    assert!(inspected.to_string().contains("Actor"));

    ai::call(
        &mut project,
        "edit_field",
        json!({ "file": GUARD, "path": ["components", "Health", "max"], "value": 80 }),
    )
    .unwrap();
    assert!(source().contains("80") && source().starts_with("// Ogre guard."));

    // Wrong types never reach the file, and the model is told where and why.
    let refused = ai::call(
        &mut project,
        "set",
        json!({
            "file": GUARD,
            "path": ["components", "Health", "max"],
            "value": "lots",
            "label": "Break health",
        }),
    )
    .unwrap_err();
    assert_eq!(refused.code, "validation");
    assert!(source().contains("80"));

    let names = ai::call(&mut project, "schema", json!({})).unwrap();
    assert!(names["components"].to_string().contains("Health"));
    let health = ai::call(&mut project, "schema", json!({ "component": "Health" })).unwrap();
    assert!(health.to_string().contains("current"));
    assert!(health.to_string().len() < 4000, "{health}");

    let wrong = ai::call(&mut project, "undo", json!({ "steps": 2 })).unwrap_err();
    assert_eq!(wrong.code, "invalid_arguments");
    ai::call(&mut project, "undo", json!({})).unwrap();
    assert_eq!(source(), original);
}

fn exchange(project: &mut AuthoringProject, messages: &[Value]) -> Vec<Value> {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let mut output = Vec::new();
    mcp::serve(project, input.as_bytes(), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn mcp_initializes_lists_and_calls_tools() {
    let (dir, mut project) = project();
    let responses = exchange(
        &mut project,
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" } } }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                "name": "edit_field",
                "arguments": { "file": GUARD, "path": ["components", "Health", "max"], "value": 90 } } }),
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
                "name": "inspect_definition", "arguments": { "path": "guards/missing" } } }),
            json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" }),
        ],
    );
    // The notification gets no response.
    assert_eq!(responses.len(), 5);
    let init = &responses[0]["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "struction");
    assert!(init["instructions"].as_str().unwrap().contains("undo"));

    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let edit = tools
        .iter()
        .find(|tool| tool["name"] == "edit_field")
        .unwrap();
    assert_eq!(
        edit["inputSchema"]["required"],
        json!(["file", "path", "value"])
    );

    assert_eq!(responses[2]["result"]["isError"], false);
    let written = std::fs::read_to_string(dir.path().join(GUARD)).unwrap();
    assert!(written.contains("90"));

    // Operation failures are tool results the model can read, not protocol errors.
    assert_eq!(responses[3]["result"]["isError"], true);
    let text = responses[3]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("guards/missing"), "{text}");

    assert_eq!(responses[4]["error"]["code"], -32601);
}

#[test]
fn mcp_answers_bad_json_and_unknown_tools_without_ending_the_session() {
    let (_dir, mut project) = project();
    let input = "{not json\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"format_disk\"}}\n{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}\n";
    let mut output = Vec::new();
    mcp::serve(&mut project, input.as_bytes(), &mut output).unwrap();
    let responses: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert_eq!(responses[1]["error"]["code"], -32602);
    assert_eq!(
        responses[2],
        json!({ "jsonrpc": "2.0", "id": 8, "result": {} })
    );
}

/// One HTTP request on its own connection: status line and body.
fn http(url: &str, request: &str) -> (String, String) {
    use std::io::{Read, Write};
    let address = url.trim_start_matches("http://").trim_end_matches("/mcp");
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

fn post(body: &Value, origin: Option<&str>) -> String {
    let body = body.to_string();
    let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
    format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{origin}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn the_http_endpoint_answers_on_the_hosts_thread_and_refuses_web_pages() {
    let (dir, mut project) = project();
    let endpoint = mcp::Endpoint::bind(0, 1).unwrap();
    let url = endpoint.url();
    let client = std::thread::spawn(move || {
        let init = http(
            &url,
            &post(
                &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": { "name": "t", "version": "0" } } }),
                Some("http://localhost:3000"),
            ),
        );
        let notified = http(
            &url,
            &post(
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                None,
            ),
        );
        let edited = http(
            &url,
            &post(
                &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                    "name": "edit_field",
                    "arguments": { "file": GUARD, "path": ["components", "Health", "max"], "value": 99 } } }),
                None,
            ),
        );
        let stream = http(&url, "GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let foreign = http(
            &url,
            &post(
                &json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" }),
                Some("https://evil.example"),
            ),
        );
        (init, notified, edited, stream, foreign)
    });
    let mut changed = false;
    while !client.is_finished() {
        changed |= endpoint.answer(Some(&mut project));
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let (init, notified, edited, stream, foreign) = client.join().unwrap();
    assert_eq!(init.0, "HTTP/1.1 200 OK");
    let init: Value = serde_json::from_str(&init.1).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "struction");
    assert_eq!(notified.0, "HTTP/1.1 202 Accepted");
    let edited: Value = serde_json::from_str(&edited.1).unwrap();
    assert_eq!(edited["result"]["isError"], false);
    assert!(changed, "the host learns the project changed");
    assert!(
        std::fs::read_to_string(dir.path().join(GUARD))
            .unwrap()
            .contains("99")
    );
    assert_eq!(stream.0, "HTTP/1.1 405 Method Not Allowed");
    assert_eq!(foreign.0, "HTTP/1.1 403 Forbidden");
}
