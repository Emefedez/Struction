//! The editor's AI side. It always serves the open project over MCP on localhost
//! (`struction_editor::mcp::Endpoint`, `http://127.0.0.1:47100/mcp` by default), so any agent can
//! inspect and edit it through the same validated, undoable operations as the GUI, and its edits
//! show up here at once. The assistant window drives the coding agents already installed and
//! signed in (Claude Code, Codex, OpenCode), as T3 Code does: each runs headless in the project
//! with that endpoint as its only way to change the project, on the user's own subscription. Which
//! agents are signed in and their models come from T3 Code's provider caches when present.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use serde_json::{Value, json};
use struction_editor::mcp::Endpoint;

use crate::state::{Command, Editor, Selected};
use crate::theme;

const EDITOR_PROMPT: &str = "You are working inside the Struction editor on the project it has \
open; the user watches your changes appear in its scene tree and inspector. Change the project \
only through the `struction` MCP tools, never by writing files or running commands, so every \
change is validated and undoable in the editor. Each user message starts with what the user has \
selected, if anything; \"this\" usually means it. Keep answers short and say what you changed.";

pub struct AssistantPlugin {
    /// Port to serve MCP from (the next free one if taken); `None` serves nothing.
    pub mcp_port: Option<u16>,
}

impl Plugin for AssistantPlugin {
    fn build(&self, app: &mut App) {
        let server = match self.mcp_port.map(|port| Endpoint::bind(port, 10)) {
            None => McpServer {
                endpoint: None,
                problem: Some("Off (started with --no-mcp)".into()),
            },
            Some(Ok(endpoint)) => McpServer {
                endpoint: Some(endpoint),
                problem: None,
            },
            Some(Err(error)) => McpServer {
                endpoint: None,
                problem: Some(format!("Could not listen: {error}")),
            },
        };
        app.insert_non_send(server)
            .insert_non_send(Assistant::default())
            .add_systems(PreUpdate, answer_mcp)
            .add_systems(
                EguiPrimaryContextPass,
                assistant_ui.after(crate::ui::editor_ui),
            );
    }
}

struct McpServer {
    endpoint: Option<Endpoint>,
    problem: Option<String>,
}

/// What the editor's header shows of the assistant, kept in sync by [`assistant_ui`].
#[derive(Default)]
pub struct Header {
    pub open: bool,
    busy: bool,
    url: Option<String>,
    problem: Option<String>,
    /// Agents and whether each is installed, for "use from other agents".
    agents: Vec<(Agent, bool)>,
    registration: Option<String>,
    /// Asked from the header; run by [`assistant_ui`].
    register: Option<Agent>,
}

impl McpServer {
    fn url(&self) -> Option<String> {
        self.endpoint.as_ref().map(Endpoint::url)
    }
}

/// Runs agents' tool calls on the editor's project between frames.
fn answer_mcp(server: NonSend<McpServer>, mut editor: NonSendMut<Editor>) {
    let Some(endpoint) = &server.endpoint else {
        return;
    };
    if endpoint.answer(editor.project.as_mut()) && !editor.playing() {
        // The scene tree, inspector and problems read the editor's snapshot.
        editor.apply(Command::Refresh);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Agent {
    Claude,
    Codex,
    OpenCode,
}

impl Agent {
    const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::OpenCode];

    fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::OpenCode => "OpenCode",
        }
    }

    fn program(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
        }
    }

    /// T3 Code's name for the provider, naming its cache file.
    fn t3_driver(self) -> &'static str {
        match self {
            Agent::Claude => "claudeAgent",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
        }
    }

    /// The agent's name for a Struction tool, minus what it adds.
    fn tool_name(self, name: &str) -> String {
        let prefix = match self {
            Agent::Claude => "mcp__struction__",
            Agent::Codex => "",
            Agent::OpenCode => "struction_",
        };
        name.strip_prefix(prefix).unwrap_or(name).to_owned()
    }

    /// The headless command for one turn: `session` continues an earlier one. Claude and Codex
    /// read the prompt from stdin; OpenCode takes it as an argument.
    fn command(
        self,
        program: &Path,
        model: &str,
        session: Option<&str>,
        url: &str,
        prompt: &str,
    ) -> Process {
        let mut command = Process::new(program);
        match self {
            Agent::Claude => {
                let config =
                    json!({ "mcpServers": { "struction": { "type": "http", "url": url } } });
                command
                    .args(["-p", "--output-format", "stream-json", "--verbose"])
                    .args(["--mcp-config", &config.to_string(), "--strict-mcp-config"])
                    // No built-in tools: the project changes only through Struction's.
                    .args(["--tools", "", "--allowedTools", "mcp__struction"])
                    .args(["--append-system-prompt", EDITOR_PROMPT]);
                if !model.is_empty() {
                    command.args(["--model", model]);
                }
                if let Some(session) = session {
                    command.args(["--resume", session]);
                }
                // A nested session would otherwise think it runs inside Claude Code.
                command.env_remove("CLAUDECODE");
            }
            Agent::Codex => {
                command.arg("exec");
                if let Some(session) = session {
                    command.args(["resume", session]);
                }
                command
                    .args(["--json", "--skip-git-repo-check"])
                    .args(["-c", &format!("mcp_servers.struction.url={url:?}")])
                    .args(["-c", "sandbox_mode=\"read-only\""])
                    .args(["-c", "approval_policy=\"never\""]);
                if !model.is_empty() {
                    command.args(["-m", model]);
                }
                command.arg("-");
            }
            Agent::OpenCode => {
                let config = json!({
                    "mcp": { "struction": { "type": "remote", "url": url, "enabled": true } }
                });
                command
                    .env("OPENCODE_CONFIG_CONTENT", config.to_string())
                    // The read-only agent: no file edits or commands, MCP tools still allowed.
                    .args(["run", "--format", "json", "--agent", "plan"]);
                if !model.is_empty() {
                    command.args(["-m", model]);
                }
                if let Some(session) = session {
                    command.args(["-s", session]);
                }
                command.args(["--", prompt]);
            }
        }
        command
    }

    fn stdin_prompt(self) -> bool {
        self != Agent::OpenCode
    }

    /// What one line of the agent's JSON output means for the transcript.
    fn parse(self, line: &str) -> Vec<Event> {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match self {
            Agent::Claude => parse_claude(self, &event),
            Agent::Codex => parse_codex(&event),
            Agent::OpenCode => parse_opencode(self, &event),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Event {
    Session(String),
    Text(String),
    ToolStarted {
        id: String,
        name: String,
        input: Value,
    },
    ToolFinished {
        id: String,
        ok: bool,
        output: String,
    },
    Notice(String),
    Failed(String),
    Finished,
}

/// Text of an MCP-style content list, or the value itself.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn parse_claude(agent: Agent, event: &Value) -> Vec<Event> {
    let mut events = Vec::new();
    match event["type"].as_str() {
        Some("system") if event["subtype"] == "init" => {
            if let Some(session) = event["session_id"].as_str() {
                events.push(Event::Session(session.to_owned()));
            }
            let struction = event["mcp_servers"]
                .as_array()
                .and_then(|servers| servers.iter().find(|s| s["name"] == "struction"));
            if let Some(server) = struction
                && server["status"] != "connected"
            {
                events.push(Event::Notice(format!(
                    "Claude Code could not connect to the editor's MCP endpoint ({})",
                    server["status"].as_str().unwrap_or("unknown")
                )));
            }
        }
        Some("assistant") => {
            for block in event["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        events.push(Event::Text(block["text"].as_str().unwrap_or("").to_owned()))
                    }
                    Some("tool_use") => events.push(Event::ToolStarted {
                        id: block["id"].as_str().unwrap_or("").to_owned(),
                        name: agent.tool_name(block["name"].as_str().unwrap_or("")),
                        input: block["input"].clone(),
                    }),
                    _ => {}
                }
            }
        }
        Some("user") => {
            for block in event["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_result" {
                    events.push(Event::ToolFinished {
                        id: block["tool_use_id"].as_str().unwrap_or("").to_owned(),
                        ok: block["is_error"] != true,
                        output: content_text(&block["content"]),
                    });
                }
            }
        }
        Some("result") if event["is_error"] == true || event["subtype"] != "success" => {
            let message = event["result"]
                .as_str()
                .or(event["subtype"].as_str())
                .unwrap_or("Claude Code failed");
            events.push(Event::Failed(message.to_owned()));
        }
        _ => {}
    }
    events
}

fn parse_codex(event: &Value) -> Vec<Event> {
    let item = &event["item"];
    let id = item["id"].as_str().unwrap_or("").to_owned();
    match (event["type"].as_str(), item["type"].as_str()) {
        (Some("thread.started"), _) => event["thread_id"]
            .as_str()
            .map(|thread| Event::Session(thread.to_owned()))
            .into_iter()
            .collect(),
        (Some("item.started"), Some("mcp_tool_call")) => vec![Event::ToolStarted {
            id,
            name: item["tool"].as_str().unwrap_or("").to_owned(),
            input: item["arguments"].clone(),
        }],
        (Some("item.completed"), Some("mcp_tool_call")) => {
            let result = &item["result"];
            let failed = item["status"] != "completed"
                || !item["error"].is_null()
                || result["isError"] == true
                || result["is_error"] == true;
            let output = if item["error"].is_null() {
                content_text(&result["content"])
            } else {
                content_text(&item["error"]["message"])
            };
            vec![Event::ToolFinished {
                id,
                ok: !failed,
                output,
            }]
        }
        (Some("item.completed"), Some("agent_message")) => {
            vec![Event::Text(item["text"].as_str().unwrap_or("").to_owned())]
        }
        (Some("item.completed"), Some("command_execution")) => vec![
            Event::ToolStarted {
                id: id.clone(),
                name: "shell".into(),
                input: item["command"].clone(),
            },
            Event::ToolFinished {
                id,
                ok: item["exit_code"] == 0,
                output: content_text(&item["aggregated_output"]),
            },
        ],
        (Some("turn.failed"), _) => vec![Event::Failed(
            event["error"]["message"]
                .as_str()
                .unwrap_or("Codex failed")
                .to_owned(),
        )],
        (Some("error"), _) => vec![Event::Failed(
            event["message"]
                .as_str()
                .unwrap_or("Codex failed")
                .to_owned(),
        )],
        _ => Vec::new(),
    }
}

fn parse_opencode(agent: Agent, event: &Value) -> Vec<Event> {
    let mut events: Vec<Event> = event["sessionID"]
        .as_str()
        .map(|session| Event::Session(session.to_owned()))
        .into_iter()
        .collect();
    let part = &event["part"];
    match event["type"].as_str() {
        Some("text") => events.push(Event::Text(part["text"].as_str().unwrap_or("").to_owned())),
        Some("tool_use") => {
            let state = &part["state"];
            let id = part["callID"].as_str().unwrap_or("").to_owned();
            events.push(Event::ToolStarted {
                id: id.clone(),
                name: agent.tool_name(part["tool"].as_str().unwrap_or("")),
                input: state["input"].clone(),
            });
            let ok = state["status"] == "completed";
            let output = if ok {
                content_text(&state["output"])
            } else {
                content_text(&state["error"])
            };
            events.push(Event::ToolFinished { id, ok, output });
        }
        Some("error") => {
            let error = &event["error"];
            let message = error["data"]["message"]
                .as_str()
                .or(error["name"].as_str())
                .unwrap_or("OpenCode failed");
            events.push(Event::Failed(message.to_owned()));
        }
        _ => {}
    }
    events
}

/// An agent on this machine.
struct Provider {
    agent: Agent,
    program: PathBuf,
    installed: bool,
    /// From T3 Code, when it has checked.
    signed_in: Option<bool>,
    /// (id, name), from T3 Code; empty means only the agent's default.
    models: Vec<(String, String)>,
}

fn providers(t3_caches: Option<&Path>) -> Vec<Provider> {
    Agent::ALL
        .into_iter()
        .map(|agent| {
            let cache = t3_caches
                .map(|dir| dir.join(format!("{}.json", agent.t3_driver())))
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            let signed_in = cache
                .as_ref()
                .and_then(|cache| cache["auth"]["status"].as_str())
                .map(|status| status == "authenticated");
            let models = cache
                .as_ref()
                .and_then(|cache| cache["models"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|model| {
                    let id = model["slug"].as_str()?;
                    Some((
                        id.to_owned(),
                        model["name"].as_str().unwrap_or(id).to_owned(),
                    ))
                })
                .collect();
            Provider {
                agent,
                program: PathBuf::from(agent.program()),
                installed: crate::programs::on_path(agent.program()),
                signed_in,
                models,
            }
        })
        .collect()
}

fn t3_caches() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".t3/caches")).filter(|dir| dir.is_dir())
}

enum Entry {
    User(String),
    Reply(String),
    Tool {
        name: String,
        input: Value,
        /// `None` while it runs.
        result: Option<(bool, String)>,
    },
    Problem(String),
}

/// A turn in progress: the agent's process and what it has said so far.
struct Run {
    agent: Agent,
    child: Child,
    events: Receiver<Event>,
    stderr: Arc<Mutex<String>>,
    /// Tool call ids to their transcript entries.
    tools: HashMap<String, usize>,
    failed: bool,
}

pub struct Assistant {
    draft: String,
    providers: Vec<Provider>,
    agent: usize,
    model: String,
    /// The agent's own conversation, continued by later messages until a new chat.
    session: Option<(Agent, String)>,
    transcript: Vec<Entry>,
    run: Option<Run>,
}

impl Default for Assistant {
    fn default() -> Self {
        let providers = providers(t3_caches().as_deref());
        // The first agent that is installed and not known to be signed out.
        let agent = providers
            .iter()
            .position(|p| p.installed && p.signed_in != Some(false))
            .unwrap_or(0);
        Self {
            draft: String::new(),
            providers,
            agent,
            model: String::new(),
            session: None,
            transcript: Vec::new(),
            run: None,
        }
    }
}

impl Assistant {
    fn send(&mut self, text: String, selected: Option<&Selected>, url: &str, root: &Path) {
        let provider = &self.providers[self.agent];
        let agent = provider.agent;
        let session = self
            .session
            .as_ref()
            .filter(|(owner, _)| *owner == agent)
            .map(|(_, id)| id.clone());
        let mut prompt = match selected {
            Some(Selected::Entity(path)) => format!("[Selected: instance `{path}`]\n"),
            Some(Selected::Definition(path)) => format!("[Selected: definition `{path}`]\n"),
            Some(Selected::Asset(path)) => format!("[Selected: model source `{path}`]\n"),
            None => String::new(),
        };
        prompt.push_str(&text);
        if session.is_none() && agent != Agent::Claude {
            // Claude Code takes it as a system prompt; the others get it with the first message.
            prompt = format!("{EDITOR_PROMPT}\n\n{prompt}");
        }
        self.transcript.push(Entry::User(text));
        let mut command = agent.command(
            &provider.program,
            &self.model,
            session.as_deref(),
            url,
            &prompt,
        );
        let stdin = agent.stdin_prompt().then_some(prompt);
        match start(agent, &mut command, stdin, root) {
            Ok(run) => self.run = Some(run),
            Err(problem) => self.transcript.push(Entry::Problem(problem)),
        }
    }

    /// Takes what the running agent said since the last frame.
    fn receive(&mut self) {
        let Some(run) = &mut self.run else {
            return;
        };
        let mut finished = false;
        while let Ok(event) = run.events.try_recv() {
            match event {
                Event::Session(id) => self.session = Some((run.agent, id)),
                Event::Text(text) => {
                    let text = text.trim();
                    if !text.is_empty() {
                        self.transcript.push(Entry::Reply(text.to_owned()));
                    }
                }
                Event::ToolStarted { id, name, input } => {
                    run.tools.insert(id, self.transcript.len());
                    self.transcript.push(Entry::Tool {
                        name,
                        input,
                        result: None,
                    });
                }
                Event::ToolFinished { id, ok, output } => {
                    if let Some(&index) = run.tools.get(&id)
                        && let Some(Entry::Tool { result, .. }) = self.transcript.get_mut(index)
                    {
                        *result = Some((ok, output));
                    }
                }
                Event::Notice(text) => self.transcript.push(Entry::Problem(text)),
                Event::Failed(text) => {
                    run.failed = true;
                    self.transcript.push(Entry::Problem(text));
                }
                Event::Finished => finished = true,
            }
        }
        if !finished {
            return;
        }
        let status = run.child.wait();
        if !run.failed && status.as_ref().is_ok_and(|status| !status.success()) {
            let stderr = run.stderr.lock().map(|s| s.clone()).unwrap_or_default();
            let last = stderr.lines().rev().find(|line| !line.trim().is_empty());
            self.transcript.push(Entry::Problem(format!(
                "{} stopped: {}",
                run.agent.label(),
                last.unwrap_or("no output")
            )));
        }
        self.run = None;
    }

    fn stop(&mut self) {
        if let Some(mut run) = self.run.take() {
            let _ = run.child.kill();
            let _ = run.child.wait();
            self.transcript.push(Entry::Problem("Stopped.".into()));
        }
    }
}

/// Starts one turn: the process runs in `root`; its JSON lines become events on a thread.
fn start(
    agent: Agent,
    command: &mut Process,
    stdin: Option<String>,
    root: &Path,
) -> Result<Run, String> {
    let mut child = command
        .current_dir(root)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start {}: {error}", agent.label()))?;
    if let (Some(prompt), Some(mut input)) = (stdin, child.stdin.take()) {
        // Written on a thread so a long prompt cannot block the editor on a full pipe.
        std::thread::spawn(move || {
            let _ = input.write_all(prompt.as_bytes());
        });
    }
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr_pipe = child.stderr.take().expect("piped stderr");
    let stderr = Arc::new(Mutex::new(String::new()));
    let collected = stderr.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
            if let Ok(mut text) = collected.lock() {
                text.push_str(&line);
                text.push('\n');
            }
        }
    });
    let (sender, events) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            for event in agent.parse(&line) {
                if sender.send(event).is_err() {
                    return;
                }
            }
        }
        let _ = sender.send(Event::Finished);
    });
    Ok(Run {
        agent,
        child,
        events,
        stderr,
        tools: HashMap::new(),
        failed: false,
    })
}

/// Registers the endpoint in an agent's own settings (user scope), so it has the editor's tools
/// wherever it runs, T3 Code included.
fn register(agent: Agent, url: &str) -> String {
    let mut command = Process::new(agent.program());
    match agent {
        Agent::Claude => command.args([
            "mcp",
            "add",
            "--transport",
            "http",
            "--scope",
            "user",
            "struction",
            url,
        ]),
        Agent::Codex => command.args(["mcp", "add", "struction", "--url", url]),
        Agent::OpenCode => {
            return format!(
                "OpenCode: add to ~/.config/opencode/opencode.json\n\"mcp\": {{ \"struction\": {{ \"type\": \"remote\", \"url\": \"{url}\" }} }}"
            );
        }
    };
    match command.env_remove("CLAUDECODE").output() {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr);
            let verdict = if output.status.success() {
                "Added to"
            } else {
                "Not added to"
            };
            format!("{verdict} {}: {}", agent.label(), text.trim())
        }
        Err(error) => format!("Could not run {}: {error}", agent.program()),
    }
}

fn assistant_ui(
    mut contexts: EguiContexts,
    mut started: Local<bool>,
    mut assistant: NonSendMut<Assistant>,
    mut toolbox: ResMut<crate::tools::Toolbox>,
    server: NonSend<McpServer>,
    editor: NonSend<Editor>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    // The theme's fonts are bound at the end of the editor's first pass.
    if !std::mem::replace(&mut *started, true) {
        return Ok(());
    }
    assistant.receive();
    let assistant = &mut *assistant;
    let url = server.url();
    let header = &mut toolbox.assistant;
    header.busy = assistant.run.is_some();
    header.url.clone_from(&url);
    header.problem.clone_from(&server.problem);
    header.agents = assistant
        .providers
        .iter()
        .map(|provider| (provider.agent, provider.installed))
        .collect();
    if let (Some(agent), Some(url)) = (header.register.take(), &url) {
        header.registration = Some(register(agent, url));
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
        header.open = !header.open;
    }
    if !header.open {
        return Ok(());
    }
    if assistant.run.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    let mut open = true;
    egui::Window::new("Assistant")
        .open(&mut open)
        .default_size([420.0, 560.0])
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(ctx.content_rect().right_top() + egui::vec2(-12.0, 60.0))
        // Above the viewport's overlays.
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let busy = assistant.run.is_some();
            agent_choice(ui, assistant, busy);
            ui.separator();

            let input_height = 86.0;
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .stick_to_bottom(true)
                .max_height((ui.available_height() - input_height).max(80.0))
                .show(ui, |ui| {
                    if assistant.transcript.is_empty() {
                        ui.label(
                            egui::RichText::new(
                                "Ask about the project or for a change: \"why does the sentry \
                                 not attack?\", \"give the player a faster roll\". The agent \
                                 works through the editor's tools, so every change is one Undo \
                                 step.",
                            )
                            .color(theme::MUTED),
                        );
                    }
                    for entry in &assistant.transcript {
                        transcript_entry(ui, entry);
                    }
                    if busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Working…").color(theme::MUTED));
                        });
                    }
                });
            ui.separator();

            let provider = &assistant.providers[assistant.agent];
            let blocker = if editor.project.is_none() {
                Some("Open a project first".to_owned())
            } else if url.is_none() {
                Some("The editor's MCP endpoint is off".to_owned())
            } else if !provider.installed {
                Some(format!("{} is not installed", provider.agent.label()))
            } else {
                None
            };
            let response = ui.add_enabled(
                !busy,
                egui::TextEdit::multiline(&mut assistant.draft)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("Message (Enter sends, Shift+Enter breaks the line)"),
            );
            let entered = response.has_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter) && !input.modifiers.shift);
            ui.horizontal(|ui| {
                let ready = !busy && blocker.is_none();
                let send = ui.add_enabled(ready, egui::Button::new("Send")).clicked();
                if (send || entered)
                    && ready
                    && !assistant.draft.trim().is_empty()
                    && let (Some(url), Some(root)) = (&url, &editor.root)
                {
                    let text = std::mem::take(&mut assistant.draft).trim().to_owned();
                    assistant.send(text, editor.selected.as_ref(), url, root);
                }
                if busy && ui.button("Stop").clicked() {
                    assistant.stop();
                }
                if !busy && !assistant.transcript.is_empty() && ui.button("New chat").clicked() {
                    assistant.transcript.clear();
                    assistant.session = None;
                }
                if let Some(blocker) = blocker {
                    ui.label(egui::RichText::new(blocker).color(theme::MUTED));
                }
            });
        });
    toolbox.assistant.open = open;
    Ok(())
}

fn agent_choice(ui: &mut egui::Ui, assistant: &mut Assistant, busy: bool) {
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal(|ui| {
            let current = &assistant.providers[assistant.agent];
            egui::ComboBox::from_id_salt("assistant agent")
                .selected_text(current.agent.label())
                .show_ui(ui, |ui| {
                    for (index, provider) in assistant.providers.iter().enumerate() {
                        let state = match (provider.installed, provider.signed_in) {
                            (false, _) => " · not installed",
                            (true, Some(false)) => " · signed out",
                            _ => "",
                        };
                        if ui
                            .selectable_label(
                                index == assistant.agent,
                                format!("{}{state}", provider.agent.label()),
                            )
                            .clicked()
                            && index != assistant.agent
                        {
                            assistant.agent = index;
                            assistant.model.clear();
                        }
                    }
                });
            let provider = &assistant.providers[assistant.agent];
            if provider.models.is_empty() {
                ui.add(
                    egui::TextEdit::singleline(&mut assistant.model)
                        .hint_text("default model")
                        .desired_width(150.0),
                );
            } else {
                let name = provider
                    .models
                    .iter()
                    .find(|(id, _)| *id == assistant.model)
                    .map_or("Default model", |(_, name)| name.as_str());
                egui::ComboBox::from_id_salt("assistant model")
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut assistant.model, String::new(), "Default model");
                        for (id, name) in &provider.models {
                            ui.selectable_value(&mut assistant.model, id.clone(), name);
                        }
                    });
            }
        });
    });
    let provider = &assistant.providers[assistant.agent];
    let note = match provider.signed_in {
        Some(true) => "Signed in (per T3 Code); runs on your own subscription.",
        Some(false) => "T3 Code reports this agent signed out; sign in with its CLI.",
        None => "Runs the installed CLI on your own account.",
    };
    ui.label(egui::RichText::new(note).small().color(theme::MUTED));
}

/// The header's MCP status and Assistant toggle, drawn by the editor's top bar.
pub fn header_buttons(ui: &mut egui::Ui, header: &mut Header) {
    let (text, color) = match &header.url {
        Some(url) => (
            format!(
                "MCP {}",
                url.trim_start_matches("http://127.0.0.1")
                    .trim_end_matches("/mcp")
            ),
            theme::WARD,
        ),
        None => ("MCP off".to_owned(), theme::MUTED),
    };
    let config = egui::containers::menu::MenuConfig::new()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    egui::containers::menu::MenuButton::new(egui::RichText::new(text).color(color))
        .config(config)
        .ui(ui, |ui| {
            ui.set_min_width(380.0);
            let Some(url) = header.url.clone() else {
                ui.label(header.problem.as_deref().unwrap_or("Off"));
                return;
            };
            ui.label(egui::RichText::new("MCP endpoint").strong());
            ui.horizontal(|ui| {
                ui.monospace(&url);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(url.clone());
                }
            });
            ui.label(
                egui::RichText::new(
                    "Any MCP client can inspect and edit the open project here, through the \
                     editor's validated, undoable operations (Streamable HTTP, this machine only).",
                )
                .small()
                .color(theme::MUTED),
            );
            ui.separator();
            ui.label(egui::RichText::new("Use from other agents").strong());
            ui.label(
                egui::RichText::new(
                    "Adds the endpoint to the agent's own settings, so it has these tools \
                     wherever it runs, T3 Code included, while the editor is open.",
                )
                .small()
                .color(theme::MUTED),
            );
            ui.horizontal_wrapped(|ui| {
                for (agent, installed) in header.agents.clone() {
                    if ui
                        .add_enabled(
                            installed,
                            egui::Button::new(format!("Add to {}", agent.label())),
                        )
                        .clicked()
                    {
                        header.register = Some(agent);
                    }
                }
            });
            if let Some(registration) = &header.registration {
                ui.add(
                    egui::Label::new(egui::RichText::new(registration).small().monospace())
                        .selectable(true),
                );
            }
        });
    let label = if header.busy {
        "Assistant …"
    } else {
        "Assistant"
    };
    if ui
        .add(egui::Button::new(label).selected(header.open))
        .on_hover_text("Ask an agent to inspect and edit this project (Ctrl+K)")
        .clicked()
    {
        header.open = !header.open;
    }
}

fn transcript_entry(ui: &mut egui::Ui, entry: &Entry) {
    match entry {
        Entry::User(text) => {
            egui::Frame::new()
                .fill(theme::PANEL)
                .corner_radius(4.0)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new(text).color(theme::ACTOR));
                });
        }
        Entry::Reply(text) => {
            ui.add(egui::Label::new(text.as_str()).selectable(true));
        }
        Entry::Tool {
            name,
            input,
            result,
        } => {
            let (mark, color) = match result {
                None => ("…", theme::MUTED),
                Some((true, _)) => ("✓", theme::WARD),
                Some((false, _)) => ("×", ui.visuals().error_fg_color),
            };
            let summary = summarize(input);
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{mark} {name} {summary}"))
                    .small()
                    .color(color),
            )
            .id_salt((name, summary.as_str(), input.to_string().len()))
            .show(ui, |ui| {
                let mut shown = |text: String| {
                    let mut text = text;
                    if text.len() > 2000 {
                        text.truncate(text.floor_char_boundary(2000));
                        text.push('…');
                    }
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).small().monospace())
                            .selectable(true),
                    );
                };
                shown(input.to_string());
                if let Some((_, output)) = result {
                    shown(output.clone());
                }
            });
        }
        Entry::Problem(text) => {
            ui.colored_label(ui.visuals().error_fg_color, text);
        }
    }
}

/// The arguments that tell calls apart, such as the path or file.
fn summarize(input: &Value) -> String {
    let Some(fields) = input.as_object() else {
        return input.as_str().unwrap_or_default().to_owned();
    };
    ["path", "file", "target", "component", "extensor"]
        .iter()
        .filter_map(|key| fields.get(*key))
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_agents_output_becomes_the_same_transcript_events() {
        // Lines as Claude Code, Codex and OpenCode printed them against the endpoint.
        let claude = Agent::Claude.parse(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Checking."},{"type":"tool_use","id":"t1","name":"mcp__struction__validate","input":{}}]}}"#,
        );
        assert_eq!(
            claude,
            [
                Event::Text("Checking.".into()),
                Event::ToolStarted {
                    id: "t1".into(),
                    name: "validate".into(),
                    input: json!({})
                }
            ]
        );
        assert_eq!(
            Agent::Claude.parse(
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"{\"diagnostics\":[]}"}]}]}}"#
            ),
            [Event::ToolFinished {
                id: "t1".into(),
                ok: true,
                output: r#"{"diagnostics":[]}"#.into()
            }]
        );
        assert_eq!(
            Agent::Codex.parse(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"mcp_tool_call","server":"struction","tool":"validate","arguments":{},"result":{"content":[{"type":"text","text":"{}"}],"isError":true},"error":null,"status":"completed"}}"#
            ),
            [Event::ToolFinished {
                id: "item_1".into(),
                ok: false,
                output: "{}".into()
            }]
        );
        assert_eq!(
            Agent::Codex.parse(r#"{"type":"thread.started","thread_id":"abc"}"#),
            [Event::Session("abc".into())]
        );
        let opencode = Agent::OpenCode.parse(
            r#"{"type":"tool_use","sessionID":"ses_1","part":{"type":"tool","tool":"struction_inspect_definition","callID":"c1","state":{"status":"completed","input":{"path":"guards/ogre"},"output":"{}"}}}"#,
        );
        assert_eq!(opencode[0], Event::Session("ses_1".into()));
        assert_eq!(
            opencode[1],
            Event::ToolStarted {
                id: "c1".into(),
                name: "inspect_definition".into(),
                input: json!({ "path": "guards/ogre" })
            }
        );
        let failed = Agent::OpenCode.parse(
            r#"{"type":"error","sessionID":"s","error":{"name":"APIError","data":{"message":"quota"}}}"#,
        );
        assert_eq!(failed[1], Event::Failed("quota".into()));
    }

    #[test]
    fn signed_in_agents_and_models_come_from_t3_codes_caches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("codex.json"),
            r#"{"auth":{"status":"authenticated"},"models":[{"slug":"gpt-x","name":"GPT X"}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("claudeAgent.json"),
            r#"{"auth":{"status":"unauthenticated"},"models":[]}"#,
        )
        .unwrap();
        let found = providers(Some(dir.path()));
        let codex = found.iter().find(|p| p.agent == Agent::Codex).unwrap();
        assert_eq!(codex.signed_in, Some(true));
        assert_eq!(codex.models, [("gpt-x".to_owned(), "GPT X".to_owned())]);
        let claude = found.iter().find(|p| p.agent == Agent::Claude).unwrap();
        assert_eq!(claude.signed_in, Some(false));
        let opencode = found.iter().find(|p| p.agent == Agent::OpenCode).unwrap();
        assert_eq!(opencode.signed_in, None);
        assert!(providers(None).iter().all(|p| p.models.is_empty()));
    }

    /// A stand-in agent prints what Claude Code would; the turn ends in the transcript.
    #[cfg(unix)]
    #[test]
    fn a_turn_runs_the_agent_and_streams_its_events_into_the_transcript() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude");
        std::fs::write(
            &script,
            r#"#!/bin/sh
cat > "$(dirname "$0")/prompt.txt"
echo '{"type":"system","subtype":"init","session_id":"s1","mcp_servers":[{"name":"struction","status":"connected"}]}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__struction__validate","input":{}}]}}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"All valid."}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"All valid.","session_id":"s1"}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut assistant = Assistant {
            agent: 0,
            ..default()
        };
        assistant.providers[0].program = script;
        assistant.providers[0].installed = true;
        assistant.send(
            "is it valid?".into(),
            Some(&Selected::Definition("guards/ogre".into())),
            "http://127.0.0.1:1/mcp",
            dir.path(),
        );
        for _ in 0..500 {
            assistant.receive();
            if assistant.run.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(assistant.run.is_none(), "the turn ends");
        assert_eq!(assistant.session, Some((Agent::Claude, "s1".into())));
        assert!(matches!(
            &assistant.transcript[1],
            Entry::Tool { name, result: Some((true, _)), .. } if name == "validate"
        ));
        assert!(matches!(&assistant.transcript[2], Entry::Reply(text) if text == "All valid."));
        let prompt = std::fs::read_to_string(dir.path().join("prompt.txt")).unwrap();
        assert!(prompt.starts_with("[Selected: definition `guards/ogre`]"));
    }
}
