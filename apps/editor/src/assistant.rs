//! The editor's assistant: a chat with Claude that inspects and edits the open project through
//! the AI tool catalog (`struction_editor::ai`), the operations the MCP server and the JSONL
//! protocol offer. Its edits are ordinary validated, undoable edits; the editor's Undo takes them
//! back. Requests run on a thread; tools run here, on the project the editor shows.
//!
//! The API key comes from `ANTHROPIC_API_KEY` or is typed in for the session; it is never saved.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use serde_json::{Value, json};
use struction_editor::ai;

use crate::state::{Command, Editor, Selected};
use crate::theme;

const API: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Tool rounds one message may take before the assistant has to ask to go on.
const MAX_ROUNDS: usize = 40;
/// Longer tool results are cut, so one listing cannot fill the context.
const MAX_RESULT: usize = 40_000;

const EDITOR_PROMPT: &str = "\n\nYou are the assistant inside the Struction editor, working on \
the project it has open; the user sees your edits appear in its scene tree and inspector. Each \
user message starts with what the user has selected, if anything; \"this\" usually means it. \
Play is controlled by the user from the editor, so the play tools are not offered here. Keep \
answers short and say what you changed.";

pub struct AssistantPlugin;

impl Plugin for AssistantPlugin {
    fn build(&self, app: &mut App) {
        app.insert_non_send(Assistant::default()).add_systems(
            EguiPrimaryContextPass,
            assistant_ui.after(crate::ui::editor_ui),
        );
    }
}

enum Entry {
    User(String),
    Reply(String),
    Tool {
        name: String,
        input: Value,
        output: String,
        ok: bool,
    },
    Problem(String),
}

pub struct Assistant {
    open: bool,
    draft: String,
    key: String,
    model: String,
    /// The conversation as the API sees it.
    messages: Vec<Value>,
    transcript: Vec<Entry>,
    waiting: Option<Receiver<Result<Value, String>>>,
    rounds: usize,
    /// Where the turn in progress starts in `messages`, and what the user asked.
    turn: (usize, String),
    tools: Value,
    /// Sends one request; tests answer without the network.
    transport: fn(&str, &Value) -> Result<Value, String>,
}

impl Default for Assistant {
    fn default() -> Self {
        let tools: Vec<Value> = ai::tools()
            .into_iter()
            .filter(|tool| !tool.play)
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                })
            })
            .collect();
        Self {
            open: false,
            draft: String::new(),
            key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
            model: std::env::var("STRUCTION_ASSISTANT_MODEL")
                .ok()
                .filter(|model| !model.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.into()),
            messages: Vec::new(),
            transcript: Vec::new(),
            waiting: None,
            rounds: 0,
            turn: (0, String::new()),
            tools: Value::Array(tools),
            transport: post,
        }
    }
}

impl Assistant {
    fn send(&mut self, text: String, selected: Option<&Selected>) {
        let context = match selected {
            Some(Selected::Entity(path)) => format!("[Selected: instance `{path}`]\n"),
            Some(Selected::Definition(path)) => format!("[Selected: definition `{path}`]\n"),
            Some(Selected::Asset(path)) => format!("[Selected: model source `{path}`]\n"),
            None => String::new(),
        };
        self.transcript.push(Entry::User(text.clone()));
        self.turn = (self.messages.len(), text.clone());
        self.messages.push(json!({
            "role": "user",
            "content": [{ "type": "text", "text": format!("{context}{text}") }],
        }));
        self.rounds = 0;
        self.request();
    }

    fn request(&mut self) {
        let mut messages = self.messages.clone();
        // Cache the conversation so far; each round only pays for what is new.
        if let Some(block) = messages
            .last_mut()
            .and_then(|message| message["content"].as_array_mut())
            .and_then(|content| content.last_mut())
        {
            block["cache_control"] = json!({ "type": "ephemeral" });
        }
        let body = json!({
            "model": self.model.trim(),
            "max_tokens": 8192,
            "system": [{
                "type": "text",
                "text": format!("{}{EDITOR_PROMPT}", ai::INSTRUCTIONS),
                "cache_control": { "type": "ephemeral" },
            }],
            "tools": self.tools,
            "messages": messages,
        });
        let key = self.key.trim().to_owned();
        let (sender, receiver) = channel();
        self.waiting = Some(receiver);
        let transport = self.transport;
        std::thread::spawn(move || {
            let _ = sender.send(transport(&key, &body));
        });
    }

    /// Takes a finished response: shows its text, runs its tool calls and asks again with their
    /// results, until the model stops calling tools.
    fn receive(&mut self, editor: &mut Editor) {
        let Some(waiting) = &self.waiting else {
            return;
        };
        let Ok(result) = waiting.try_recv() else {
            return;
        };
        self.waiting = None;
        let response = match result {
            Ok(response) => response,
            Err(problem) => {
                self.abandon_turn();
                self.transcript.push(Entry::Problem(problem));
                return;
            }
        };
        let content = response["content"].clone();
        self.messages
            .push(json!({ "role": "assistant", "content": content }));
        let mut results = Vec::new();
        let mut edited = false;
        for block in content.as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or_default().trim();
                    if !text.is_empty() {
                        self.transcript.push(Entry::Reply(text.to_owned()));
                    }
                }
                Some("tool_use") => {
                    let name = block["name"].as_str().unwrap_or_default().to_owned();
                    let input = block["input"].clone();
                    let (output, ok) = match editor.project.as_mut() {
                        None => ("No project is open".to_owned(), false),
                        Some(project) => match ai::call(project, &name, input.clone()) {
                            Ok(result) => (result.to_string(), true),
                            Err(failure) => (json!(failure).to_string(), false),
                        },
                    };
                    edited |= ok && !READS.contains(&name.as_str());
                    let mut text = output.clone();
                    if text.len() > MAX_RESULT {
                        text.truncate(text.floor_char_boundary(MAX_RESULT));
                        text.push_str("\n[cut: ask for something narrower]");
                    }
                    results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": block["id"],
                        "content": text,
                        "is_error": !ok,
                    }));
                    self.transcript.push(Entry::Tool {
                        name,
                        input,
                        output,
                        ok,
                    });
                }
                _ => {}
            }
        }
        if edited {
            // The scene tree, inspector and problems read the editor's snapshot.
            editor.apply(Command::Refresh);
        }
        if response["stop_reason"] == "max_tokens" {
            self.transcript
                .push(Entry::Problem("The reply hit its length limit.".into()));
        }
        if results.is_empty() {
            return;
        }
        self.messages
            .push(json!({ "role": "user", "content": results }));
        self.rounds += 1;
        if self.rounds >= MAX_ROUNDS {
            self.messages
                .push(json!({ "role": "assistant", "content": [{ "type": "text",
                "text": "I stopped after many tool calls; say continue to go on." }] }));
            self.transcript.push(Entry::Problem(format!(
                "Stopped after {MAX_ROUNDS} rounds of tool calls. Say \"continue\" to go on."
            )));
            return;
        }
        self.request();
    }

    fn stop(&mut self) {
        if self.waiting.take().is_some() {
            self.abandon_turn();
            self.transcript.push(Entry::Problem("Stopped.".into()));
        }
    }

    /// Takes the unfinished turn out of the conversation, whose tool calls may lack results, and
    /// puts the question back in the draft to send again. Edits it made stay, each undoable.
    fn abandon_turn(&mut self) {
        let (start, text) = std::mem::take(&mut self.turn);
        self.messages.truncate(start);
        if self.draft.trim().is_empty() {
            self.draft = text;
        }
    }
}

/// Tools that never change the project, so they need no refresh.
const READS: [&str; 13] = [
    "validate",
    "definitions",
    "definition_hierarchy",
    "inspect_definition",
    "entities",
    "master_hierarchy",
    "inspect_entity",
    "read",
    "source_location",
    "field_options",
    "actions",
    "schema",
    "history",
];

fn post(key: &str, body: &Value) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into();
    let mut response = agent
        .post(API)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .send(body.to_string())
        .map_err(|error| format!("Could not reach the Anthropic API: {error}"))?;
    let status = response.status();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("Reading the reply failed: {error}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|_| format!("Unexpected reply ({status}): {text}"))?;
    if !status.is_success() {
        let message = value["error"]["message"].as_str().unwrap_or(&text);
        return Err(format!("{status}: {message}"));
    }
    Ok(value)
}

fn assistant_ui(
    mut contexts: EguiContexts,
    mut started: Local<bool>,
    mut assistant: NonSendMut<Assistant>,
    mut editor: NonSendMut<Editor>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    // The theme's fonts are bound at the end of the editor's first pass.
    if !std::mem::replace(&mut *started, true) {
        return Ok(());
    }
    assistant.receive(&mut editor);

    // The header's free right end.
    egui::Area::new("assistant toggle".into())
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 8.0))
        .show(ctx, |ui| {
            let label = if assistant.waiting.is_some() {
                "Assistant …"
            } else {
                "Assistant"
            };
            if ui
                .selectable_label(assistant.open, label)
                .on_hover_text("Ask Claude to inspect and edit this project (Ctrl+K)")
                .clicked()
            {
                assistant.open = !assistant.open;
            }
        });
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
        assistant.open = !assistant.open;
    }
    if !assistant.open {
        return Ok(());
    }
    if assistant.waiting.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    let mut open = true;
    let assistant = &mut *assistant;
    egui::Window::new("Assistant")
        .open(&mut open)
        .default_size([420.0, 560.0])
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(ctx.content_rect().right_top() + egui::vec2(-12.0, 60.0))
        // Above the viewport's overlays.
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::CollapsingHeader::new("Model and key")
                .default_open(assistant.key.trim().is_empty())
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Model");
                        ui.text_edit_singleline(&mut assistant.model);
                    });
                    ui.horizontal(|ui| {
                        ui.label("API key");
                        ui.add(
                            egui::TextEdit::singleline(&mut assistant.key)
                                .password(true)
                                .hint_text("sk-ant-…"),
                        );
                    });
                    ui.label(
                        egui::RichText::new(
                            "Kept for this session only; set ANTHROPIC_API_KEY to skip this.",
                        )
                        .small()
                        .color(theme::MUTED),
                    );
                });
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
                                 not attack?\", \"give the player a faster roll\". Every change \
                                 it makes is one Undo step.",
                            )
                            .color(theme::MUTED),
                        );
                    }
                    for entry in &assistant.transcript {
                        transcript_entry(ui, entry);
                    }
                    if assistant.waiting.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Working…").color(theme::MUTED));
                        });
                    }
                });
            ui.separator();

            let busy = assistant.waiting.is_some();
            let ready = !busy && !assistant.key.trim().is_empty() && editor.project.is_some();
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
                let send = ui.add_enabled(ready, egui::Button::new("Send")).clicked();
                if (send || entered) && ready && !assistant.draft.trim().is_empty() {
                    let text = std::mem::take(&mut assistant.draft).trim().to_owned();
                    assistant.send(text, editor.selected.as_ref());
                }
                if busy && ui.button("Stop").clicked() {
                    assistant.stop();
                }
                if !busy && !assistant.transcript.is_empty() && ui.button("New chat").clicked() {
                    assistant.messages.clear();
                    assistant.transcript.clear();
                }
                if editor.project.is_none() {
                    ui.label(egui::RichText::new("Open a project first").color(theme::MUTED));
                } else if assistant.key.trim().is_empty() {
                    ui.label(egui::RichText::new("Needs an API key").color(theme::MUTED));
                }
            });
        });
    assistant.open = open;
    Ok(())
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
            output,
            ok,
        } => {
            let (mark, color) = if *ok {
                ("✓", theme::WARD)
            } else {
                ("×", ui.visuals().error_fg_color)
            };
            let summary = summarize(input);
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{mark} {name} {summary}"))
                    .small()
                    .color(color),
            )
            .id_salt((name, summary.as_str(), output.len()))
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
                shown(output.clone());
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
        return String::new();
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
    fn stopping_drops_the_unanswered_turn_so_it_can_be_resent() {
        let mut assistant = Assistant {
            key: "test".into(),
            ..default()
        };
        assistant.messages = vec![
            json!({ "role": "user", "content": [{ "type": "text", "text": "hi" }] }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "hello" }] }),
            json!({ "role": "user", "content": [{ "type": "text", "text": "rename it" }] }),
            json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "a", "name": "read", "input": {} }] }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "a", "content": "{}" }] }),
        ];
        assistant.turn = (2, "rename it".into());
        let (_sender, receiver) = channel();
        assistant.waiting = Some(receiver);
        assistant.stop();
        assert_eq!(assistant.messages.len(), 2);
        assert_eq!(assistant.messages[1]["role"], "assistant");
        assert_eq!(assistant.draft, "rename it");
    }

    #[test]
    fn tools_run_on_the_open_project_and_their_results_go_back() {
        let dir = tempfile::tempdir().unwrap();
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/authoring");
        copy(&root, dir.path());
        let mut editor = Editor::default();
        editor.apply(Command::Open(dir.path().into()));
        let mut assistant = Assistant {
            transport: |_, body| {
                // The second round carries the tool results and gets the closing reply.
                let last = &body["messages"].as_array().unwrap().last().unwrap()["content"];
                assert_eq!(last[0]["type"], "tool_result");
                Ok(json!({
                    "content": [{ "type": "text", "text": "Done." }],
                    "stop_reason": "end_turn",
                }))
            },
            ..default()
        };
        assert!(
            !assistant.tools.to_string().contains("\"start_play\""),
            "play stays with the editor's own controls"
        );
        let file = "guards/ogre/entity.jsonc";
        let (sender, receiver) = channel();
        assistant.waiting = Some(receiver);
        sender
            .send(Ok(json!({
                "content": [
                    { "type": "text", "text": "Raising it." },
                    { "type": "tool_use", "id": "t1", "name": "edit_field",
                      "input": { "file": file, "path": ["components", "Health", "max"], "value": 75 } },
                    { "type": "tool_use", "id": "t2", "name": "launch_rockets", "input": {} },
                ],
                "stop_reason": "tool_use",
            })))
            .unwrap();
        assistant.receive(&mut editor);
        let source = || std::fs::read_to_string(dir.path().join(file)).unwrap();
        assert!(source().contains("75"));
        let results = &assistant.messages.last().unwrap()["content"];
        assert_eq!(results[0]["tool_use_id"], "t1");
        assert_eq!(results[0]["is_error"], false);
        assert_eq!(results[1]["is_error"], true);
        // The results went back; wait for the closing reply.
        let reply = assistant.waiting.as_ref().unwrap().recv().unwrap();
        let (sender, receiver) = channel();
        sender.send(reply).unwrap();
        assistant.waiting = Some(receiver);
        assistant.receive(&mut editor);
        assert!(assistant.waiting.is_none());
        assert!(matches!(assistant.transcript.last(), Some(Entry::Reply(text)) if text == "Done."));
        // The editor's own Undo takes the assistant's edit back.
        editor.apply(Command::Undo);
        assert!(!source().contains("75"));
    }

    fn copy(from: &std::path::Path, to: &std::path::Path) {
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&target).unwrap();
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
}
