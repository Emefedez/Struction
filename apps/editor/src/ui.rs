//! Panels around the viewport. Widgets read the editor snapshot and emit `Command`s, applied
//! after the pass, so every change goes through the shared authoring operations.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bevy::{camera::Viewport, prelude::*, window::PrimaryWindow};
use bevy_egui::{
    EguiContext, EguiContexts,
    egui::{
        self, Align, Color32, DragValue, Frame, Key, KeyboardShortcut, Layout, Margin, Modifiers,
        RichText, Sense, TextEdit, Ui, UiBuilder,
    },
};
use serde_json::Value;
use struction_editor::{Diagnostic, EditRequest, Field};

use crate::state::{
    Command, Editor, Inspection, Selected, definition_file, entity_key, lookup, quat, vec3,
};
use crate::theme;
use crate::viewport::SceneCamera;

/// Text fields that must survive between passes.
#[derive(Default)]
pub struct Drafts {
    open_path: String,
    new_definition: String,
}

#[allow(clippy::too_many_arguments)]
pub fn editor_ui(
    mut contexts: EguiContexts,
    mut themed: Local<bool>,
    mut drafts: Local<Drafts>,
    mut editor: NonSendMut<Editor>,
    mut typing: ResMut<Typing>,
    mut camera: Single<&mut Camera, (With<SceneCamera>, Without<EguiContext>)>,
    window: Single<&Window, With<PrimaryWindow>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !*themed {
        // New fonts load at the end of this pass; drawing now would use unbound families.
        theme::apply(ctx);
        *themed = true;
        return Ok(());
    }
    let mut commands = Vec::new();
    typing.0 = ctx.text_edit_focused();
    shortcuts(ctx, &editor, &mut commands);

    let mut root = Ui::new(
        ctx.clone(),
        "editor".into(),
        UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    let bar = Frame::new()
        .fill(theme::BASE)
        .inner_margin(Margin::symmetric(14, 8));
    let top = egui::Panel::top("top_bar")
        .frame(bar)
        .show(&mut root, |ui| top_bar(ui, &editor, &mut commands))
        .response
        .rect
        .height();

    let panel = Frame::new()
        .fill(theme::PANEL)
        .inner_margin(Margin::same(12));
    let left = egui::Panel::left("hierarchy")
        .resizable(true)
        .default_size(260.0)
        .size_range(180.0..=480.0)
        .frame(panel)
        .show(&mut root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                hierarchy(ui, &editor, &mut drafts, &mut commands);
            });
            ui.allocate_rect(ui.available_rect_before_wrap(), Sense::hover());
        })
        .response
        .rect
        .width();

    let right = egui::Panel::right("inspector")
        .resizable(true)
        .default_size(340.0)
        .size_range(260.0..=560.0)
        .frame(panel)
        .show(&mut root, |ui| {
            ui.label(theme::section("Inspector"));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                inspector(ui, &mut editor, &mut commands);
            });
            ui.allocate_rect(ui.available_rect_before_wrap(), Sense::hover());
        })
        .response
        .rect
        .width();

    let bottom = egui::Panel::bottom("problems")
        .resizable(true)
        .default_size(150.0)
        .frame(panel)
        .show(&mut root, |ui| {
            problems(ui, &editor, &mut commands);
            ui.allocate_rect(ui.available_rect_before_wrap(), Sense::hover());
        })
        .response
        .rect
        .height();

    if editor.project.is_none() {
        let free = egui::Rect::from_min_max(
            egui::pos2(left, top),
            ctx.viewport_rect().max - egui::vec2(right, bottom),
        );
        welcome(&mut root, free, &mut drafts, &mut commands);
    }

    // Panel sizes are logical; the viewport is in physical pixels.
    let scale = window.scale_factor();
    let position = UVec2::new((left * scale) as u32, (top * scale) as u32);
    let size = UVec2::new(window.physical_width(), window.physical_height())
        .saturating_sub(position)
        .saturating_sub(UVec2::new((right * scale) as u32, (bottom * scale) as u32));
    camera.viewport = (size.x > 0 && size.y > 0).then(|| Viewport {
        physical_position: position,
        physical_size: size,
        ..default()
    });

    for command in commands {
        editor.apply(command);
    }
    Ok(())
}

/// Whether a text field has the keyboard; viewport keys yield to typing only, not to a
/// focused button or list row.
#[derive(Resource, Default)]
pub struct Typing(pub bool);

fn shortcuts(ctx: &egui::Context, editor: &Editor, commands: &mut Vec<Command>) {
    if ctx.text_edit_focused() || editor.project.is_none() {
        return;
    }
    let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
    let redo = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    let redo_y = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
    let play = KeyboardShortcut::new(Modifiers::COMMAND, Key::P);
    ctx.input_mut(|input| {
        // Longer shortcuts first: Ctrl+Z would also match Ctrl+Shift+Z.
        if input.consume_shortcut(&redo) || input.consume_shortcut(&redo_y) {
            commands.push(Command::Redo);
        } else if input.consume_shortcut(&undo) {
            commands.push(Command::Undo);
        }
        if input.consume_shortcut(&play) {
            commands.push(if editor.playing() {
                Command::StopPlay
            } else {
                Command::StartPlay
            });
        }
    });
}

fn top_bar(ui: &mut Ui, editor: &Editor, commands: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("STRUCTION")
                .heading()
                .extra_letter_spacing(2.0),
        );
        ui.label(RichText::new("●").color(theme::ACCENT).small());
        ui.add_space(12.0);
        let Some(project) = &editor.project else {
            ui.label(RichText::new("No project open").color(theme::MUTED));
            return;
        };
        let root = editor.root.as_ref().map(|root| root.display().to_string());
        ui.label(RichText::new(root.unwrap_or_default()).color(theme::MUTED));

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(play) = &editor.play {
                if ui.button("Stop").clicked() {
                    commands.push(Command::StopPlay);
                }
                if ui.button("Step").clicked() {
                    commands.push(Command::Step);
                }
                let label = if play.running { "Pause" } else { "Resume" };
                if ui.button(label).clicked() {
                    commands.push(Command::TogglePause);
                }
                ui.label(
                    RichText::new(format!("PLAYING · tick {}", play.ticks))
                        .color(theme::ACCENT)
                        .monospace(),
                );
            } else {
                let valid = editor.diagnostics.is_empty();
                let play = ui
                    .add_enabled(valid, egui::Button::new("▶ Play"))
                    .on_hover_text("Run the game from current sources (Ctrl+P)")
                    .on_disabled_hover_text("Play requires valid sources: see Problems");
                if play.clicked() {
                    commands.push(Command::StartPlay);
                }
            }
            ui.separator();
            let history = project.session().history();
            let editing = !editor.playing();
            let redo = ui
                .add_enabled(editing && history.can_redo(), egui::Button::new("Redo"))
                .on_hover_text(history.redo_label().map_or("Nothing to redo".into(), |l| {
                    format!("Redo {l} (Ctrl+Shift+Z)")
                }));
            if redo.clicked() {
                commands.push(Command::Redo);
            }
            let undo = ui
                .add_enabled(editing && history.can_undo(), egui::Button::new("Undo"))
                .on_hover_text(
                    history
                        .undo_label()
                        .map_or("Nothing to undo".into(), |l| format!("Undo {l} (Ctrl+Z)")),
                );
            if undo.clicked() {
                commands.push(Command::Undo);
            }
            if ui
                .add_enabled(editing, egui::Button::new("Refresh"))
                .on_hover_text("Revalidate sources edited outside the editor")
                .clicked()
            {
                commands.push(Command::Refresh);
            }
        });
    });
}

fn welcome(root: &mut Ui, free: egui::Rect, drafts: &mut Drafts, commands: &mut Vec<Command>) {
    let size = egui::vec2(420.0, 150.0);
    let rect = egui::Rect::from_center_size(free.center(), size);
    root.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        Frame::new()
            .fill(theme::PANEL)
            .corner_radius(10)
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.label(theme::section("Open project"));
                ui.label(
                    RichText::new("A directory with definitions and scenes/. Edits save to it.")
                        .color(theme::MUTED),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let field = ui.add(
                        TextEdit::singleline(&mut drafts.open_path)
                            .hint_text("/path/to/project")
                            .desired_width(300.0),
                    );
                    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if (ui.button("Open").clicked() || entered) && !drafts.open_path.is_empty() {
                        commands.push(Command::Open(PathBuf::from(drafts.open_path.trim())));
                    }
                });
            });
    });
}

/// Authored paths form a tree; zones and spawners without an entity appear as folders.
#[derive(Default)]
struct Node<'a> {
    entity: Option<&'a Value>,
    children: BTreeMap<&'a str, Node<'a>>,
}

fn hierarchy(ui: &mut Ui, editor: &Editor, drafts: &mut Drafts, commands: &mut Vec<Command>) {
    ui.label(theme::section("Scene"));
    ui.add_space(4.0);
    if editor.project.is_none() {
        ui.label(RichText::new("Open a project to see its scene.").color(theme::MUTED));
        return;
    }
    let mut tree = Node::default();
    let mut runtime = Vec::new();
    for entity in &editor.entities {
        let Some(path) = entity["path"].as_str() else {
            runtime.push(entity);
            continue;
        };
        let node = path.split('/').fold(&mut tree, |node, part| {
            node.children.entry(part).or_default()
        });
        node.entity = Some(entity);
    }
    ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
        if tree.children.is_empty() {
            ui.label(RichText::new("No authored spawns.").color(theme::MUTED));
        }
        for (name, node) in &tree.children {
            tree_node(ui, name, node, editor, commands);
        }
        for entity in runtime {
            entity_row(ui, entity_key(entity), entity, editor, commands);
        }

        ui.add_space(14.0);
        ui.label(theme::section("Definitions"));
        ui.add_space(4.0);
        for path in &editor.definitions {
            let selected = editor.selected == Some(Selected::Definition(path.clone()));
            if ui.selectable_label(selected, path).clicked() {
                commands.push(Command::Select(Some(Selected::Definition(path.clone()))));
            }
        }
    });
    ui.add_space(8.0);
    // New definitions descend from the selected one, or the first root.
    let parent = match &editor.selected {
        Some(Selected::Definition(path)) => Some(path.clone()),
        _ => editor.definitions.first().cloned(),
    };
    if let Some(parent) = parent {
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut drafts.new_definition)
                    .hint_text("new/definition")
                    .desired_width(ui.available_width() - 60.0),
            );
            let create = ui
                .add_enabled(
                    !drafts.new_definition.trim().is_empty() && !editor.playing(),
                    egui::Button::new("New"),
                )
                .on_hover_text(format!("Create a definition descending from {parent}"));
            if create.clicked() {
                let path = drafts.new_definition.trim().to_owned();
                commands.push(Command::CreateDefinition {
                    path: path.clone(),
                    parent,
                });
                commands.push(Command::Select(Some(Selected::Definition(path))));
                drafts.new_definition.clear();
            }
        });
    }
}

fn tree_node(ui: &mut Ui, name: &str, node: &Node, editor: &Editor, commands: &mut Vec<Command>) {
    if node.children.is_empty() {
        if let Some(entity) = node.entity {
            entity_row(ui, name, entity, editor, commands);
        }
        return;
    }
    let id = ui.make_persistent_id(("tree", name, node.entity.map(entity_key)));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| match node.entity {
            Some(entity) => entity_row(ui, name, entity, editor, commands),
            None => {
                ui.label(RichText::new(name).color(theme::MUTED));
            }
        })
        .body(|ui| {
            for (child, node) in &node.children {
                tree_node(ui, child, node, editor, commands);
            }
        });
}

fn entity_row(
    ui: &mut Ui,
    name: &str,
    entity: &Value,
    editor: &Editor,
    commands: &mut Vec<Command>,
) {
    let key = entity_key(entity).to_owned();
    let selected = editor.selected == Some(Selected::Entity(key.clone()));
    let mut text = RichText::new(name);
    if entity["disabled"].as_bool() == Some(true) {
        text = text.color(theme::MUTED).italics();
    }
    let row = ui.selectable_label(selected, text);
    let row = match entity["definition"].as_str() {
        Some(definition) => row.on_hover_text(definition),
        None => row,
    };
    if row.clicked() {
        commands.push(Command::Select(Some(Selected::Entity(key))));
    }
}

fn inspector(ui: &mut Ui, editor: &mut Editor, commands: &mut Vec<Command>) {
    let playing = editor.playing();
    let Some(inspection) = editor.inspection() else {
        ui.label(RichText::new("Select an entity or definition.").color(theme::MUTED));
        return;
    };
    match inspection {
        Inspection::Missing(target) => {
            ui.label(RichText::new(format!("{target} no longer exists.")).color(theme::MUTED));
        }
        Inspection::Entity {
            entry,
            components,
            authored,
            unavailable,
            spawn,
            overrides,
        } => {
            let key = entity_key(entry);
            ui.heading(key.rsplit('/').next().unwrap_or(key));
            ui.label(RichText::new(key).monospace().small().color(theme::MUTED));
            ui.add_space(6.0);
            egui::Grid::new("entity_facts")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    if let Some(definition) = entry["definition"].as_str() {
                        fact(ui, "Definition");
                        if ui.link(definition).clicked() {
                            commands.push(Command::Select(Some(Selected::Definition(
                                definition.to_owned(),
                            ))));
                        }
                        ui.end_row();
                    }
                    if let Some(source) = entry["source"].as_object() {
                        fact(ui, "Source");
                        let file = source["file"].as_str().unwrap_or_default();
                        let line = source["line"].as_u64().unwrap_or_default();
                        mono(ui, &format!("{file}:{line}"));
                        ui.end_row();
                    }
                    if let Some(id) = entry["stable_id"].as_str() {
                        fact(ui, "Stable id");
                        mono(ui, id);
                        ui.end_row();
                    }
                    if let Some(master) = entry["master"].as_str() {
                        fact(ui, "Master");
                        mono(ui, master);
                        ui.end_row();
                    }
                });
            ui.add_space(10.0);

            let editable = spawn.is_some() && !playing;
            section(ui, "Transform", |ui| {
                if let Some(position) = vec3(&entry["position"]) {
                    let edited = vector_row(ui, "Position", position, editable, 0.05);
                    if let Some((position, done)) = edited {
                        if position != vec3(&entry["position"]).unwrap_or(position) {
                            commands.push(Command::Move {
                                path: key.to_owned(),
                                position,
                                group: Some(format!("inspector-move:{key}")),
                            });
                        }
                        if done {
                            commands.push(Command::EndGroup);
                        }
                    }
                }
                if let Some(rotation) = quat(&entry["rotation"]) {
                    let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
                    // Rounded, and plus zero, so float noise does not show as -0.000.
                    let degrees = (Vec3::new(x, y, z) * 180_000.0 / std::f32::consts::PI).round()
                        / 1000.0
                        + Vec3::ZERO;
                    vector_row(ui, "Rotation", degrees, false, 0.0);
                }
                if let Some(scale) = vec3(&entry["scale"]) {
                    vector_row(ui, "Scale", scale, false, 0.0);
                }
                if !editable && !playing {
                    hint(ui, "Only named spawns can move.");
                }
            });

            // Placement is the Transform section above; it moves the spawn, not an override.
            let components = components.iter().filter(|(name, _)| name != "Transform");
            for (name, value) in components.clone().filter(|(n, _)| authored.contains(n)) {
                // Instance edits are scene overrides on this spawn.
                let target = spawn.as_ref().filter(|_| !playing).map(|(file, path)| {
                    let mut path = path.clone();
                    path.extend(
                        ["overrides", "components", name.as_str()].map(|k| Field::Key(k.into())),
                    );
                    FieldTarget {
                        file: file.clone(),
                        path,
                        authored: overrides.get("components").and_then(|c| c.get(name)),
                        owner: key.rsplit('/').next().unwrap_or(key).to_owned(),
                    }
                });
                section(ui, name, |ui| {
                    value_editor(ui, value, &mut Vec::new(), target.as_ref(), commands);
                });
            }
            let runtime: Vec<_> = components
                .filter(|(name, _)| !authored.contains(name))
                .collect();
            if !runtime.is_empty() || !unavailable.is_empty() {
                egui::CollapsingHeader::new(theme::section("Runtime"))
                    .id_salt("runtime")
                    .default_open(false)
                    .show(ui, |ui| {
                        for (name, value) in runtime {
                            ui.label(RichText::new(name).strong());
                            ui.indent(name, |ui| {
                                value_editor(ui, value, &mut Vec::new(), None, commands);
                            });
                        }
                        for name in unavailable {
                            ui.label(RichText::new(name).color(theme::MUTED))
                                .on_hover_text("Not reflected: no value to show");
                        }
                    });
            }
        }
        Inspection::Definition {
            path,
            lineage,
            components,
            local,
        } => {
            ui.heading(path.as_str());
            let file = definition_file(path);
            ui.label(RichText::new(&file).monospace().small().color(theme::MUTED));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                fact(ui, "Lineage");
                for (index, ancestor) in lineage.iter().enumerate() {
                    if index > 0 {
                        ui.label(RichText::new("›").color(theme::MUTED));
                    }
                    if ancestor == path {
                        ui.label(ancestor);
                    } else if ui.link(ancestor).clicked() {
                        commands.push(Command::Select(Some(Selected::Definition(
                            ancestor.clone(),
                        ))));
                    }
                }
            });
            ui.add_space(10.0);
            if components.is_empty() {
                hint(ui, "No components.");
            }
            for (name, value) in components {
                // Definitions author Transform as their `transform` section.
                let (path_in_file, authored) = if name == "Transform" {
                    (vec![Field::Key("transform".into())], local.get("transform"))
                } else {
                    (
                        ["components", name.as_str()]
                            .map(|k| Field::Key(k.into()))
                            .to_vec(),
                        local.get("components").and_then(|c| c.get(name)),
                    )
                };
                let target = (!playing).then(|| FieldTarget {
                    file: file.clone(),
                    path: path_in_file,
                    authored,
                    owner: path.clone(),
                });
                section(ui, name, |ui| {
                    value_editor(ui, value, &mut Vec::new(), target.as_ref(), commands);
                });
            }
        }
    }
}

/// Where a component's fields are written, and what that source already sets.
struct FieldTarget<'a> {
    file: String,
    path: Vec<Field>,
    authored: Option<&'a Value>,
    owner: String,
}

impl FieldTarget<'_> {
    fn full_path(&self, field: &[Field]) -> Vec<Field> {
        self.path.iter().chain(field).cloned().collect()
    }
    fn is_set(&self, field: &[Field]) -> bool {
        self.authored
            .is_some_and(|authored| lookup(authored, field).is_some())
    }
    fn label(&self, verb: &str, field: &[Field]) -> String {
        let component = match self.path.last() {
            Some(Field::Key(key)) => key.as_str(),
            _ => "",
        };
        let names: Vec<String> = field.iter().map(field_name).collect();
        format!("{verb} {component}.{} on {}", names.join("."), self.owner)
    }
    fn set(&self, field: &[Field], value: Value, group: bool) -> Command {
        let path = self.full_path(field);
        Command::Edit(EditRequest::Set {
            group: group.then(|| format!("field:{}:{path:?}", self.file)),
            file: self.file.clone(),
            label: self.label("Set", field),
            value,
            path,
            revision: None,
        })
    }
    fn reset(&self, field: &[Field]) -> Command {
        Command::Edit(EditRequest::Remove {
            file: self.file.clone(),
            path: self.full_path(field),
            label: self.label("Reset", field),
            revision: None,
        })
    }
}

fn field_name(field: &Field) -> String {
    match field {
        Field::Key(key) => key.clone(),
        Field::Index(index) => index.to_string(),
    }
}

/// Renders a reflected value; with a target, leaves are editable and write to its source.
fn value_editor(
    ui: &mut Ui,
    value: &Value,
    field: &mut Vec<Field>,
    target: Option<&FieldTarget>,
    commands: &mut Vec<Command>,
) {
    match value {
        Value::Object(members) => {
            for (key, member) in members {
                field.push(Field::Key(key.clone()));
                if member.is_object() || member.as_array().is_some_and(|a| !is_vector(a)) {
                    ui.label(RichText::new(key).color(theme::MUTED));
                    ui.indent(key, |ui| value_editor(ui, member, field, target, commands));
                } else {
                    leaf_row(ui, key, member, field, target, commands);
                }
                field.pop();
            }
            if members.is_empty() {
                hint(ui, "No fields.");
            }
        }
        Value::Array(items) if !is_vector(items) => {
            for (index, item) in items.iter().enumerate() {
                field.push(Field::Index(index));
                leaf_row(ui, &index.to_string(), item, field, target, commands);
                field.pop();
            }
        }
        leaf => leaf_row(ui, "value", leaf, field, target, commands),
    }
}

fn is_vector(items: &[Value]) -> bool {
    (2..=4).contains(&items.len()) && items.iter().all(Value::is_number)
}

fn leaf_row(
    ui: &mut Ui,
    label: &str,
    value: &Value,
    field: &[Field],
    target: Option<&FieldTarget>,
    commands: &mut Vec<Command>,
) {
    ui.horizontal(|ui| {
        let set_here = target.is_some_and(|t| t.is_set(field));
        // Amber marks a field this source sets; others are inherited or defaults.
        let marker = if set_here {
            theme::ACCENT
        } else {
            Color32::TRANSPARENT
        };
        ui.label(RichText::new("●").small().color(marker));
        ui.add_sized(
            [90.0, 22.0],
            egui::Label::new(RichText::new(label).color(theme::MUTED)).truncate(),
        );
        let editable = target.is_some();
        // The reset button's room is kept even when absent, so fields line up.
        let reset_room = 30.0;
        let width = ui.available_width() - reset_room;
        let edited = ui
            .add_enabled_ui(editable, |ui| leaf_widget(ui, value, field, width))
            .inner;
        if let (Some(target), Some((new, done))) = (target, edited) {
            if &new != value {
                commands.push(target.set(field, new, true));
            }
            if done {
                commands.push(Command::EndGroup);
            }
        }
        if let Some(target) = target.filter(|_| set_here) {
            let reset = ui
                .small_button("↺")
                .on_hover_text("Remove this value here to inherit it again");
            if reset.clicked() {
                commands.push(target.reset(field));
            }
        }
    });
}

/// A new value and whether the interaction finished (drag released or focus lost).
fn leaf_widget(ui: &mut Ui, value: &Value, field: &[Field], width: f32) -> Option<(Value, bool)> {
    match value {
        Value::Bool(flag) => {
            let mut flag = *flag;
            ui.checkbox(&mut flag, "")
                .changed()
                .then(|| (Value::Bool(flag), true))
        }
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                let mut integer = integer;
                let response = ui.add(DragValue::new(&mut integer).speed(0.2));
                finished(&response, response.changed()).map(|done| (Value::from(integer), done))
            } else {
                let mut float = number.as_f64().unwrap_or_default();
                let response = ui.add(DragValue::new(&mut float).speed(0.1).max_decimals(3));
                finished(&response, response.changed()).map(|done| (Value::from(float), done))
            }
        }
        Value::String(text) => {
            // Edited in a draft and written once, so partial text is never validated.
            let id = ui.id().with(("draft", format!("{field:?}")));
            let mut draft = ui
                .data_mut(|d| d.get_temp::<String>(id))
                .unwrap_or(text.clone());
            let response = ui.add(TextEdit::singleline(&mut draft).desired_width(width.min(200.0)));
            if response.has_focus() {
                ui.data_mut(|d| d.insert_temp(id, draft.clone()));
            }
            if response.lost_focus() {
                ui.data_mut(|d| d.remove::<String>(id));
                return (&draft != text).then(|| (Value::String(draft), true));
            }
            None
        }
        Value::Array(items) if is_vector(items) => {
            let vector: Vec<f32> = items
                .iter()
                .filter_map(|v| v.as_f64())
                .map(|v| v as f32)
                .collect();
            let mut padded = [0.0; 3];
            for (slot, value) in padded.iter_mut().zip(&vector) {
                *slot = *value;
            }
            if vector.len() == 3 {
                return vector_fields(ui, Vec3::from(padded), true, 0.05, width)
                    .map(|(v, done)| (Value::from(v.to_array().to_vec()), done));
            }
            ui.label(RichText::new(format!("{vector:?}")).monospace());
            None
        }
        Value::Null => {
            ui.label(RichText::new("none").color(theme::MUTED));
            None
        }
        other => {
            ui.label(RichText::new(other.to_string()).monospace());
            None
        }
    }
}

fn finished(response: &egui::Response, changed: bool) -> Option<bool> {
    let done = response.drag_stopped() || response.lost_focus();
    (changed || done).then_some(done)
}

/// A labelled vector with axis-colored components.
fn vector_row(
    ui: &mut Ui,
    label: &str,
    value: Vec3,
    editable: bool,
    speed: f64,
) -> Option<(Vec3, bool)> {
    ui.horizontal(|ui| {
        ui.add_sized(
            [64.0, 22.0],
            egui::Label::new(RichText::new(label).color(theme::MUTED)),
        );
        let total = ui.available_width();
        vector_fields(ui, value, editable, speed, total)
    })
    .inner
}

/// Three fields sharing `total` width; sized exactly so rows never widen their panel.
fn vector_fields(
    ui: &mut Ui,
    value: Vec3,
    editable: bool,
    speed: f64,
    total: f32,
) -> Option<(Vec3, bool)> {
    let gap = ui.spacing().item_spacing.x;
    let width = ((total - 2.0 * gap) / 3.0).floor().max(40.0);
    let mut result = value;
    let mut changed = false;
    let mut done = false;
    for (axis, color) in theme::AXES.into_iter().enumerate() {
        let mut component = value[axis] as f64;
        let response = ui.add_enabled_ui(editable, |ui| {
            ui.add_sized(
                [width, 24.0],
                DragValue::new(&mut component).speed(speed).max_decimals(3),
            )
        });
        let response = response.inner;
        let bar =
            egui::Rect::from_min_size(response.rect.min, [3.0, response.rect.height()].into());
        ui.painter().rect_filled(
            bar,
            egui::CornerRadius {
                nw: 5,
                sw: 5,
                ..Default::default()
            },
            color,
        );
        if response.changed() {
            result[axis] = component as f32;
            changed = true;
        }
        done |= response.drag_stopped() || response.lost_focus();
    }
    (changed || done).then_some((result, done))
}

fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(theme::section(title))
        .id_salt(title)
        .default_open(true)
        .show(ui, body);
    ui.add_space(4.0);
}

fn fact(ui: &mut Ui, label: &str) {
    ui.label(RichText::new(label).color(theme::MUTED));
}

fn mono(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).monospace());
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::MUTED));
}

fn problems(ui: &mut Ui, editor: &Editor, commands: &mut Vec<Command>) {
    let count = editor.diagnostics.len();
    ui.horizontal(|ui| {
        ui.label(theme::section("Problems"));
        let color = if count > 0 {
            ui.visuals().error_fg_color
        } else {
            theme::MUTED
        };
        ui.label(RichText::new(count.to_string()).small().color(color));
        if let Some(status) = &editor.status {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(status).small().color(theme::MUTED));
            });
        }
    });
    ui.add_space(4.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        if let Some(rejection) = &editor.rejection {
            let warn = ui.visuals().warn_fg_color;
            ui.label(
                RichText::new(format!(
                    "{} rejected: {}",
                    rejection.action, rejection.message
                ))
                .color(warn),
            );
            ui.indent("rejection", |ui| {
                for diagnostic in &rejection.diagnostics {
                    diagnostic_row(ui, diagnostic, warn, editor, commands);
                }
            });
        }
        if editor.project.is_some() && count == 0 && editor.rejection.is_none() {
            ui.label(RichText::new("All sources are valid.").color(theme::MUTED));
        }
        let error = ui.visuals().error_fg_color;
        let unique: BTreeSet<_> = editor
            .diagnostics
            .iter()
            .map(|d| (d.file.clone(), d.line, d.column, d.message.clone()))
            .collect();
        for (file, line, column, message) in unique {
            let diagnostic = Diagnostic {
                message,
                file,
                line,
                column,
            };
            diagnostic_row(ui, &diagnostic, error, editor, commands);
        }
    });
}

fn diagnostic_row(
    ui: &mut Ui,
    diagnostic: &Diagnostic,
    color: Color32,
    editor: &Editor,
    commands: &mut Vec<Command>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("●").small().color(color));
        if let Some(file) = &diagnostic.file {
            let location = match (diagnostic.line, diagnostic.column) {
                (Some(line), Some(column)) => format!("{file}:{line}:{column}"),
                (Some(line), None) => format!("{file}:{line}"),
                _ => file.clone(),
            };
            // Definition files select their definition; scene files have no single target.
            let definition = file.strip_suffix("/entity.jsonc");
            let known = definition.is_some_and(|d| editor.definitions.iter().any(|p| p == d));
            if known {
                if ui.link(RichText::new(location).monospace()).clicked() {
                    commands.push(Command::Select(Some(Selected::Definition(
                        definition.unwrap_or_default().to_owned(),
                    ))));
                }
            } else {
                ui.label(RichText::new(location).monospace().color(theme::MUTED));
            }
        }
        ui.label(&diagnostic.message);
    });
}
