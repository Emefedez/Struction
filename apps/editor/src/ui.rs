//! Panels around the viewport. Widgets read the editor snapshot and emit `Command`s, applied
//! after the pass, so every change goes through the shared authoring operations.
use std::collections::BTreeMap;
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
use struction_editor::{Diagnostic, EditRequest, EntityEntry, ExtensorWhy, Field, HierarchyNode};

use crate::state::{Command, Editor, Extensors, Inspection, Selected, definition_file, lookup};
use crate::theme;
use crate::tools::{self, Mode, Request, Toolbox};
use crate::viewport::SceneCamera;

/// Text fields that must survive between passes.
pub struct Drafts {
    open_path: String,
    new_definition: String,
    new_actor: String,
    actor_definition: String,
    actor_spawner: String,
    actor_master: Option<String>,
    by_master: bool,
    compact_tab: u8,
}

impl Default for Drafts {
    fn default() -> Self {
        Self {
            open_path: String::new(),
            new_definition: String::new(),
            new_actor: String::new(),
            actor_definition: String::new(),
            actor_spawner: String::new(),
            actor_master: None,
            by_master: true,
            compact_tab: 0,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn editor_ui(
    mut contexts: EguiContexts,
    mut themed: Local<bool>,
    mut drafts: Local<Drafts>,
    mut editor: NonSendMut<Editor>,
    mut toolbox: ResMut<Toolbox>,
    models: Res<struction_scene::render::Models>,
    mut typing: ResMut<Typing>,
    mut guides: ResMut<crate::spatial_guides::GuideSettings>,
    control: Res<crate::play_view::PlayControl>,
    camera_transform: Single<&GlobalTransform, With<SceneCamera>>,
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
    let mut top = egui::Panel::top("top_bar")
        .frame(bar)
        .show(&mut root, |ui| {
            top_bar(ui, &editor, &mut toolbox, &mut commands)
        })
        .response
        .rect
        .height();

    let panel = Frame::new()
        .fill(theme::PANEL)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .inner_margin(Margin::same(12));
    let bottom = egui::Panel::bottom("problems")
        .resizable(true)
        .default_size(if editor.diagnostics.is_empty() {
            64.0
        } else {
            150.0
        })
        .frame(panel)
        .show(&mut root, |ui| {
            problems(ui, &editor, &mut commands);
            for (path, error) in &models.errors {
                ui.colored_label(theme::AXES[0], format!("Model {path}: {error}"));
            }
            ui.allocate_rect(ui.available_rect_before_wrap(), Sense::hover());
        })
        .response
        .rect
        .height();
    let compact = ctx.viewport_rect().width() < 800.0;
    let (left, right) = if compact && editor.project.is_some() {
        top += egui::Panel::top("compact_tabs")
            .frame(panel)
            .show(&mut root, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut drafts.compact_tab, 0, "Scene");
                    ui.selectable_value(&mut drafts.compact_tab, 1, "Inspector");
                    ui.selectable_value(&mut drafts.compact_tab, 2, "Viewport");
                });
            })
            .response
            .rect
            .height();
        if drafts.compact_tab != 2 {
            let size = root.available_size();
            panel.show(&mut root, |ui| {
                ui.set_min_size((size - egui::vec2(24.0, 24.0)).max(egui::Vec2::ZERO));
                if drafts.compact_tab == 1 {
                    inspector_header(ui, &editor, &mut toolbox);
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if drafts.compact_tab == 0 {
                        hierarchy(ui, &editor, &mut drafts, &mut commands);
                        assets(ui, &editor, &mut toolbox, &mut commands);
                    } else {
                        inspector(ui, &mut editor, &mut toolbox, &mut commands);
                    }
                });
                ui.allocate_rect(ui.available_rect_before_wrap(), Sense::hover());
            });
        }
        (0.0, 0.0)
    } else if editor.project.is_some() {
        let left = egui::Panel::left("hierarchy")
            .resizable(true)
            .default_size(260.0)
            .size_range(180.0..=480.0)
            .frame(panel)
            .show(&mut root, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    hierarchy(ui, &editor, &mut drafts, &mut commands);
                    assets(ui, &editor, &mut toolbox, &mut commands);
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
                inspector_header(ui, &editor, &mut toolbox);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    inspector(ui, &mut editor, &mut toolbox, &mut commands);
                });
                ui.allocate_rect(ui.available_rect_before_wrap(), Sense::hover());
            })
            .response
            .rect
            .width();

        (left, right)
    } else {
        (0.0, 0.0)
    };

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
    camera.is_active = !(compact && editor.project.is_some() && drafts.compact_tab != 2);
    camera.viewport = (size.x > 0 && size.y > 0).then(|| Viewport {
        physical_position: position,
        physical_size: size,
        ..default()
    });

    if camera.is_active {
        let free = egui::Rect::from_min_max(
            egui::pos2(left, top),
            ctx.viewport_rect().max - egui::vec2(right, bottom),
        );
        viewport_overlay(
            ctx,
            free,
            &editor,
            &control,
            &mut guides,
            &camera,
            &camera_transform,
        );
    }
    for command in commands {
        if compact && matches!(&command, Command::StartPlay | Command::TogglePause) {
            drafts.compact_tab = 2;
        }
        if compact && matches!(&command, Command::Select(Some(_))) {
            drafts.compact_tab = 1;
        }
        if let Command::OpenIde(target) = command {
            if let Some(project) = &editor.project {
                match project
                    .source_location(&target)
                    .map_err(|e| e.to_string())
                    .and_then(|location| toolbox.programs.open_ide(&location))
                {
                    Ok(()) => editor.status = Some("Opened source in IDE".into()),
                    Err(message) => {
                        editor.rejection = Some(crate::state::Rejection {
                            action: "Open in IDE",
                            message,
                            diagnostics: Vec::new(),
                        })
                    }
                }
            }
        } else {
            editor.apply(command);
        }
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

fn top_bar(ui: &mut Ui, editor: &Editor, toolbox: &mut Toolbox, commands: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("STRUCTION")
                .heading()
                .extra_letter_spacing(2.0),
        );
        ui.label(RichText::new("●").color(theme::ACCENT).small());
        ui.add_space(12.0);
        ui.menu_button("Toolbox", |ui| {
            ui.set_min_width(250.0);
            ui.label(theme::section("Model tools").color(theme::ASSET));
            if toolbox.assets.is_empty() {
                hint(ui, "Open a project to see its model sources.");
            }
            for asset in toolbox.assets.clone() {
                ui.menu_button(asset.rsplit('/').next().unwrap_or(&asset), |ui| {
                    ui.label(RichText::new(&asset).small().color(theme::MUTED));
                    for mode in Mode::ALL {
                        if ui.button(mode.label()).clicked() {
                            toolbox.requests.push(Request::Open {
                                asset: asset.clone(),
                                mode,
                            });
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
            if ui.button("Rescan assets").clicked() {
                toolbox.rescan();
            }
        });
        ui.menu_button("Programs…", |ui| {
            ui.set_min_width(360.0);
            ui.label(RichText::new("IDE command").strong());
            ui.add(TextEdit::singleline(&mut toolbox.programs.ide_input).desired_width(360.0));
            hint(ui, "Example: code --goto {file}:{line}:{column}");
            hint(
                ui,
                "Quote paths with spaces. No shell expansion. A missing {file} is appended.",
            );
            if ui.button("Save IDE command").clicked()
                && let Err(error) = toolbox.programs.save_ide()
            {
                toolbox.programs.error = Some(error);
            }
            ui.separator();
            ui.label(RichText::new("Blender executable").strong());
            ui.add(TextEdit::singleline(&mut toolbox.programs.input).desired_width(360.0));
            ui.horizontal(|ui| {
                if ui.button("Browse…").clicked()
                    && let Some(path) = crate::programs::picker(&toolbox.programs.input)
                {
                    toolbox.programs.input = path.to_string_lossy().into_owned();
                }
                if ui
                    .add_enabled(toolbox.tool.is_none(), egui::Button::new("Save"))
                    .clicked()
                {
                    let path = PathBuf::from(toolbox.programs.input.trim());
                    if let Err(error) = toolbox.programs.select(path, true) {
                        toolbox.programs.error = Some(error);
                    }
                }
                if ui.button("Detect").clicked() {
                    toolbox.programs.input = struction_assets::Blender::default()
                        .executable
                        .to_string_lossy()
                        .into_owned();
                }
            });
            hint(ui, "Used for mesh import, preparation and Open in Blender.");
            if toolbox.tool.is_some() {
                hint(
                    ui,
                    "Close the mesh tool before saving a different executable.",
                );
            }
            if let Some(error) = &toolbox.programs.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(status) = &toolbox.programs.status {
                ui.colored_label(theme::WARD, status);
            }
            ui.label(format!(
                "Current: {}",
                toolbox.programs.blender.executable.display()
            ));
        });
    });
    let Some(project) = &editor.project else {
        ui.label(RichText::new("No project open").color(theme::MUTED));
        return;
    };
    let root = editor.root.as_ref().map(|root| root.display().to_string());
    ui.add(
        egui::Label::new(RichText::new(root.unwrap_or_default()).color(theme::MUTED)).truncate(),
    );

    ui.horizontal_wrapped(|ui| {
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
            let valid = editor.diagnostics.is_empty() && !editor.definitions.is_empty();
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
            .add_enabled(editing, egui::Button::new("Open…"))
            .on_hover_text("Open another project folder")
            .clicked()
            && let Some(folder) = pick_project()
        {
            commands.push(Command::Open(folder));
        }
        if ui
            .add_enabled(editing, egui::Button::new("Refresh"))
            .on_hover_text("Revalidate sources edited outside the editor")
            .clicked()
        {
            commands.push(Command::Refresh);
        }
    });
}

/// The system's folder picker. It blocks this frame while open, like any modal dialog.
fn pick_project() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open a Struction project")
        .pick_folder()
}

fn welcome(root: &mut Ui, free: egui::Rect, drafts: &mut Drafts, commands: &mut Vec<Command>) {
    let size = egui::vec2(460.0, 150.0);
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
                            .desired_width(260.0),
                    );
                    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if (ui.button("Open").clicked() || entered) && !drafts.open_path.is_empty() {
                        commands.push(Command::Open(PathBuf::from(drafts.open_path.trim())));
                    }
                    if ui
                        .button("Browse…")
                        .on_hover_text("Choose the project folder")
                        .clicked()
                        && let Some(folder) = pick_project()
                    {
                        drafts.open_path = folder.display().to_string();
                        commands.push(Command::Open(folder));
                    }
                });
            });
    });
}

/// Authored paths form a tree; zones and spawners without an entity appear as folders.
#[derive(Default)]
struct Node<'a> {
    entity: Option<&'a EntityEntry>,
    children: BTreeMap<&'a str, Node<'a>>,
}

fn hierarchy(ui: &mut Ui, editor: &Editor, drafts: &mut Drafts, commands: &mut Vec<Command>) {
    ui.label(theme::section("Scene"));
    ui.horizontal_wrapped(|ui| {
        for (label, color) in [
            ("Instance", theme::ACTOR),
            ("Master", theme::MASTER),
            ("Ward", theme::WARD),
            ("Definition", theme::DEFINITION),
        ] {
            ui.label(RichText::new(label).small().color(color));
        }
    });
    ui.add_space(4.0);
    if editor.project.is_none() {
        ui.label(RichText::new("Open a project to see its scene.").color(theme::MUTED));
        return;
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut drafts.by_master, false, "Placement");
        ui.selectable_value(&mut drafts.by_master, true, "Masters & wards");
    });
    actor_creator(ui, editor, drafts, commands);
    let mut tree = Node::default();
    let mut runtime = Vec::new();
    for entity in &editor.entities {
        let Some(path) = entity.path.as_deref() else {
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
        if drafts.by_master {
            for node in &editor.masters {
                master_node(ui, node, editor, commands);
            }
        } else {
            for (name, node) in &tree.children {
                tree_node(ui, name, node, editor, commands);
            }
            for entity in runtime {
                entity_row(ui, entity.key(), entity, editor, commands);
            }
        }

        ui.add_space(14.0);
        ui.label(theme::section("Definitions").color(theme::DEFINITION));
        ui.add_space(4.0);
        hint(ui, "Inheritance (descendsFrom)");
        for node in &editor.lineages {
            definition_node(ui, node, editor, commands);
        }
    });
    ui.add_space(8.0);
    // New definitions descend from the selected one, or the first root.
    let parent = match &editor.selected {
        Some(Selected::Definition(path)) => Some(path.clone()),
        _ => editor.definitions.first().cloned(),
    };
    if let Some(parent) = parent {
        ui.label(format!("New definition descends from: {parent}"));
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

fn master_node(ui: &mut Ui, node: &HierarchyNode, editor: &Editor, commands: &mut Vec<Command>) {
    let Some(entity) = editor.entity(&node.key) else {
        return;
    };
    let name = node.key.rsplit('/').next().unwrap_or(&node.key);
    if node.children.is_empty() {
        entity_row(ui, name, entity, editor, commands);
        return;
    }
    egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.make_persistent_id(("master", &node.key)),
        true,
    )
    .show_header(ui, |ui| entity_row(ui, name, entity, editor, commands))
    .body(|ui| {
        for child in &node.children {
            master_node(ui, child, editor, commands);
        }
    });
}

fn definition_node(
    ui: &mut Ui,
    node: &HierarchyNode,
    editor: &Editor,
    commands: &mut Vec<Command>,
) {
    let row = |ui: &mut Ui, commands: &mut Vec<Command>| {
        let row = ui.selectable_label(
            editor.selected == Some(Selected::Definition(node.key.clone())),
            RichText::new(&node.key).color(
                if editor
                    .diagnostics
                    .iter()
                    .any(|d| d.file.as_deref() == Some(definition_file(&node.key).as_str()))
                {
                    ui.visuals().error_fg_color
                } else {
                    theme::DEFINITION
                },
            ),
        );
        row.context_menu(|ui| {
            if ui.button("Open in IDE").clicked() {
                commands.push(Command::OpenIde(
                    struction_editor::SourceTarget::Definition(node.key.clone()),
                ));
                ui.close();
            }
        });
        if row.clicked() {
            commands.push(Command::Select(Some(Selected::Definition(
                node.key.clone(),
            ))));
        }
    };
    if node.children.is_empty() {
        row(ui, commands);
        return;
    }
    egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.make_persistent_id(("definition", &node.key)),
        true,
    )
    .show_header(ui, |ui| row(ui, commands))
    .body(|ui| {
        for child in &node.children {
            definition_node(ui, child, editor, commands);
        }
    });
}

fn master_choices(
    ui: &mut Ui,
    nodes: &[HierarchyNode],
    selected: &mut Option<String>,
    excluded: Option<&str>,
    depth: usize,
) {
    for node in nodes {
        // A ward cannot become its own master or adopt an ancestor.
        if excluded == Some(node.key.as_str()) {
            continue;
        }
        ui.selectable_value(
            selected,
            Some(node.key.clone()),
            format!("{}{}", "  ".repeat(depth), node.key),
        );
        master_choices(ui, &node.children, selected, excluded, depth + 1);
    }
}

fn definition_choices(ui: &mut Ui, nodes: &[HierarchyNode], selected: &mut String, depth: usize) {
    for node in nodes {
        ui.selectable_value(
            selected,
            node.key.clone(),
            RichText::new(format!("{}{}", "  ".repeat(depth), node.key)).color(theme::DEFINITION),
        );
        definition_choices(ui, &node.children, selected, depth + 1);
    }
}

fn actor_creator(ui: &mut Ui, editor: &Editor, drafts: &mut Drafts, commands: &mut Vec<Command>) {
    egui::CollapsingHeader::new("New actor (instance)").show(ui, |ui| {
        ui.add_enabled_ui(!editor.playing(), |ui| {
            ui.add(TextEdit::singleline(&mut drafts.new_actor).hint_text("Actor name"));
            let definition = if drafts.actor_definition.is_empty() {
                "Choose definition"
            } else { &drafts.actor_definition };
            egui::ComboBox::from_id_salt("actor_definition")
                .selected_text(definition)
                .show_ui(ui, |ui| {
                    definition_choices(ui, &editor.lineages, &mut drafts.actor_definition, 0);
                });
            let spawner = if drafts.actor_spawner.is_empty() {
                "Choose placement spawner"
            } else { &drafts.actor_spawner };
            egui::ComboBox::from_id_salt("actor_spawner")
                .selected_text(spawner)
                .show_ui(ui, |ui| {
                    for entity in &editor.entities {
                        if entity.source.as_ref().is_some_and(|s| s.path.len() == 2) {
                            ui.selectable_value(&mut drafts.actor_spawner, entity.key().to_owned(), entity.key());
                        }
                    }
                });
            ui.label("Master (masterIs)");
            egui::ComboBox::from_id_salt("actor_master")
                .selected_text(drafts.actor_master.as_deref().unwrap_or("None — independent actor"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut drafts.actor_master, None, "None — independent actor");
                    master_choices(ui, &editor.masters, &mut drafts.actor_master, None, 0);
                });
            hint(ui, "The new actor will be a ward beneath this master. Placement stays with the chosen spawner.");
            let ready = !drafts.new_actor.trim().is_empty()
                && !drafts.actor_definition.is_empty()
                && !drafts.actor_spawner.is_empty();
            if ui.add_enabled(ready, egui::Button::new("Create actor")).clicked() {
                let name = drafts.new_actor.trim().to_owned();
                commands.push(Command::CreateSpawn {
                    spawner: drafts.actor_spawner.clone(), name: name.clone(),
                    definition: drafts.actor_definition.clone(), master: drafts.actor_master.clone(),
                });
                commands.push(Command::Select(Some(Selected::Entity(format!("{}/{name}", drafts.actor_spawner)))));
                drafts.by_master = true;
            }
        });
    });
}

fn tree_node(ui: &mut Ui, name: &str, node: &Node, editor: &Editor, commands: &mut Vec<Command>) {
    if node.children.is_empty() {
        if let Some(entity) = node.entity {
            entity_row(ui, name, entity, editor, commands);
        }
        return;
    }
    let id = ui.make_persistent_id(("tree", name, node.entity.map(EntityEntry::key)));
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
    entity: &EntityEntry,
    editor: &Editor,
    commands: &mut Vec<Command>,
) {
    let key = entity.key().to_owned();
    let selected = editor.selected == Some(Selected::Entity(key.clone()));
    let master = editor
        .entities
        .iter()
        .any(|e| e.master.as_deref() == Some(key.as_str()));
    let (role, color) = if master && entity.master.is_some() {
        ("master + ward", theme::MASTER)
    } else if master {
        ("master", theme::MASTER)
    } else if entity.master.is_some() {
        ("ward", theme::WARD)
    } else if entity.is_instance() {
        ("instance", theme::ACTOR)
    } else {
        ("placement", theme::MUTED)
    };
    let mut text = RichText::new(format!("{name} · {role}")).color(color);
    if entity.disabled {
        text = text.color(theme::MUTED).italics();
    }
    let row = ui.selectable_label(selected, text);
    let row = match &entity.definition {
        Some(definition) => row.on_hover_text(format!(
            "{key}\nDefinition: {definition}\nMaster: {}",
            entity.master.as_deref().unwrap_or("none")
        )),
        None => row.on_hover_text(&key),
    };
    row.context_menu(|ui| {
        if ui
            .add_enabled(entity.source.is_some(), egui::Button::new("Open in IDE"))
            .clicked()
        {
            commands.push(Command::OpenIde(struction_editor::SourceTarget::Entity(
                key.clone(),
            )));
            ui.close();
        }
    });
    if row.clicked() {
        commands.push(Command::Select(Some(Selected::Entity(key))));
    }
}

/// Mesh sources in the project; each opens in the mesh tool.
fn assets(ui: &mut Ui, editor: &Editor, toolbox: &mut Toolbox, commands: &mut Vec<Command>) {
    if editor.project.is_none() {
        return;
    }
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.label(theme::section("Assets").color(theme::ASSET));
        if ui
            .small_button("↻")
            .on_hover_text("Look for new mesh sources")
            .clicked()
        {
            toolbox.rescan();
        }
    });
    ui.add_space(4.0);
    if toolbox.assets.is_empty() {
        hint(
            ui,
            "No .blend, .gltf or .glb sources in the asset directory.",
        );
        return;
    }
    ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
        for asset in &toolbox.assets {
            let selected = editor.selected == Some(Selected::Asset(asset.clone()));
            let row = ui
                .selectable_label(selected, asset.as_str())
                .on_hover_text("Double-click to open in the mesh tool");
            if row.clicked() {
                commands.push(Command::Select(Some(Selected::Asset(asset.clone()))));
            }
            if row.double_clicked() {
                toolbox.requests.push(Request::Open {
                    asset: asset.clone(),
                    mode: Mode::Inspect,
                });
            }
        }
    });
}

fn inspector_header(ui: &mut Ui, editor: &Editor, toolbox: &mut Toolbox) {
    let Some(selected) = &editor.selected else {
        return;
    };
    let (path, color) = match selected {
        Selected::Asset(path) => (path, theme::ASSET),
        Selected::Entity(path) => (path, theme::ACTOR),
        Selected::Definition(path) => (path, theme::DEFINITION),
    };
    ui.colored_label(
        color,
        RichText::new(path.rsplit('/').next().unwrap_or(path)).heading(),
    );
    ui.label(RichText::new(path).small().monospace().color(theme::MUTED));
    if let Selected::Asset(asset) = selected {
        ui.horizontal_wrapped(|ui| {
            for mode in Mode::ALL {
                if ui.small_button(mode.label()).clicked() {
                    toolbox.requests.push(Request::Open {
                        asset: asset.clone(),
                        mode,
                    });
                }
            }
        });
    }
    ui.separator();
}

fn asset_inspector(ui: &mut Ui, asset: &str, editor: &Editor, toolbox: &mut Toolbox) {
    let Some(source) = editor
        .root
        .as_ref()
        .map(|root| tools::asset_source(root, asset))
    else {
        return;
    };
    let recipe = struction_assets::recipe_path(&source).is_file();
    let compiled = struction_assets::compile::default_output(&source).is_file();
    egui::Grid::new("asset_facts")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            fact(ui, "Recipe");
            ui.label(if recipe {
                "Custom settings"
            } else {
                "Defaults"
            });
            ui.end_row();
            fact(ui, "Compiled");
            ui.label(if compiled { "Yes" } else { "Not yet" });
            ui.end_row();
        });
    ui.add_space(8.0);
    let (mut open, mut external) = (None, false);
    ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
        if ui
            .button("Inspect mesh…")
            .on_hover_text("Open the mesh tool: bounds, counts and a 3D preview")
            .clicked()
        {
            open = Some(Mode::Inspect);
        }
        if ui
            .button("Generate LODs…")
            .on_hover_text("Preview levels of detail from presets, then apply")
            .clicked()
        {
            open = Some(Mode::Lods);
        }
        if ui
            .button("Generate collision…")
            .on_hover_text("Preview hull, trimesh and convex parts from presets, then apply")
            .clicked()
        {
            open = Some(Mode::Collision);
        }
        ui.add_space(4.0);
        if ui
            .button(tools::open_in_label(&source))
            .on_hover_text("Edit the source in its full application")
            .clicked()
        {
            external = true;
        }
    });
    if let Some(mode) = open {
        toolbox.requests.push(Request::Open {
            asset: asset.to_owned(),
            mode,
        });
    }
    if external {
        toolbox
            .requests
            .push(Request::OpenExternally(asset.to_owned()));
    }
    if let Some(error) = &toolbox.open_error {
        ui.label(
            RichText::new(error)
                .small()
                .color(ui.visuals().error_fg_color),
        );
    }
}

fn inspector(ui: &mut Ui, editor: &mut Editor, toolbox: &mut Toolbox, commands: &mut Vec<Command>) {
    if let Some(Selected::Asset(asset)) = editor.selected.clone() {
        asset_inspector(ui, &asset, editor, toolbox);
        return;
    }
    if let Some(selected) = &editor.selected {
        let target = match selected {
            Selected::Definition(path) => {
                Some(struction_editor::SourceTarget::Definition(path.clone()))
            }
            Selected::Entity(path) => Some(struction_editor::SourceTarget::Entity(path.clone())),
            Selected::Asset(_) => None,
        };
        if let Some(target) = target {
            let location = editor
                .project
                .as_ref()
                .and_then(|p| p.source_location(&target).ok());
            if ui
                .add_enabled(location.is_some(), egui::Button::new("Open in IDE"))
                .on_hover_text(location.map_or_else(
                    || "No authored source".into(),
                    |s| format!("{}:{}", s.file.display(), s.line),
                ))
                .clicked()
            {
                commands.push(Command::OpenIde(target));
            }
        }
    }
    let playing = editor.playing();
    let masters = editor.masters.clone();
    let schema = editor.schema.clone();
    let Some(inspection) = editor.inspection() else {
        ui.label(RichText::new("Select an entity or definition.").color(theme::MUTED));
        return;
    };
    match inspection {
        // Drawn by `asset_inspector` above.
        Inspection::Asset => {}
        Inspection::Missing(target) => {
            ui.label(
                RichText::new(format!(
                    "No instance at {target}. It may not have spawned; see Problems."
                ))
                .color(theme::MUTED),
            );
        }
        Inspection::InvalidDefinition {
            path,
            message,
            source,
        } => {
            ui.heading(RichText::new(path.as_str()).color(ui.visuals().error_fg_color));
            ui.label(message.as_str());
            let file = definition_file(path);
            ui.label(RichText::new(&file).monospace());
            if ui.button("Open source in editor…").clicked() {
                toolbox.requests.push(Request::OpenSource(file));
            }
            hint(
                ui,
                "Fix the source, then Refresh. This definition remains listed while invalid.",
            );
            egui::ScrollArea::both().show(ui, |ui| {
                ui.monospace(source.as_str());
            });
        }
        Inspection::Entity {
            entry,
            components,
            authored,
            unavailable,
            spawn,
            overrides,
        } => {
            let key = entry.key();
            ui.heading(RichText::new(key.rsplit('/').next().unwrap_or(key)).color(
                if entry.master.is_some() {
                    theme::WARD
                } else {
                    theme::ACTOR
                },
            ));
            ui.label(RichText::new(key).monospace().small().color(theme::MUTED));
            ui.add_space(6.0);
            egui::Grid::new("entity_facts")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    if let Some(definition) = &entry.definition {
                        fact(ui, "Definition");
                        if ui.link(definition).clicked() {
                            commands.push(Command::Select(Some(Selected::Definition(
                                definition.clone(),
                            ))));
                        }
                        ui.end_row();
                    }
                    if let Some(source) = &entry.source {
                        fact(ui, "Source");
                        mono(ui, &format!("{}:{}", source.file, source.line));
                        ui.end_row();
                    }
                    if let Some(id) = &entry.stable_id {
                        fact(ui, "Stable id");
                        mono(ui, id);
                        ui.end_row();
                    }
                    fact(ui, "Master (masterIs)");
                    let mut master = entry.master.clone();
                    ui.add_enabled_ui(entry.is_named_spawn() && !playing, |ui| {
                        egui::ComboBox::from_id_salt(("master", key))
                            .selected_text(master.as_deref().unwrap_or("None — independent"))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut master, None, "None — independent");
                                master_choices(ui, &masters, &mut master, Some(key), 0);
                            });
                    });
                    if master != entry.master {
                        commands.push(Command::SetMaster {
                            path: key.into(),
                            master,
                        });
                    }
                    ui.end_row();
                });
            ui.add_space(10.0);

            let editable = spawn.is_some() && !playing;
            section(ui, "Transform", |ui| {
                if let Some(current) = entry.position {
                    let edited = vector_row(ui, "Position", current, editable, 0.05);
                    if let Some((position, done)) = edited {
                        if position != current {
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
                if let Some(rotation) = entry.rotation {
                    let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
                    // Rounded, and plus zero, so float noise does not show as -0.000.
                    let degrees = (Vec3::new(x, y, z) * 180_000.0 / std::f32::consts::PI).round()
                        / 1000.0
                        + Vec3::ZERO;
                    vector_row(ui, "Rotation", degrees, false, 0.0);
                }
                if let Some(scale) = entry.scale {
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
                        schema: &schema,
                        component_schema: &schema["properties"]["components"]["properties"][name],
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
            extensors,
            library,
            overridden,
        } => {
            ui.heading(RichText::new(path.as_str()).color(theme::DEFINITION));
            let file = definition_file(path);
            ui.label(RichText::new(&file).monospace().small().color(theme::MUTED));
            match library {
                Some(library) if *overridden => {
                    hint(ui, &format!("Overrides the {library} definition."));
                }
                Some(library) => hint(
                    ui,
                    &format!("From the read-only {library} library; editing creates this file."),
                ),
                None => {}
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                fact(ui, "Lineage");
                // The lineage starts at the primordial type; the definition itself ends the chain
                // unless an older inspection already included it.
                let mut chain: Vec<&str> = lineage.iter().map(String::as_str).collect();
                if !chain.contains(&path.as_str()) {
                    chain.push(path);
                }
                for (index, ancestor) in chain.iter().enumerate() {
                    if index > 0 {
                        ui.label(RichText::new("›").color(theme::MUTED));
                    }
                    if *ancestor == path {
                        ui.label(*ancestor);
                    } else if ui.link(*ancestor).clicked() {
                        commands.push(Command::Select(Some(Selected::Definition(
                            (*ancestor).to_owned(),
                        ))));
                    }
                }
            });
            ui.add_space(6.0);
            extensor_section(ui, path, extensors, playing, commands);
            ui.add_space(6.0);
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
                    schema: &schema,
                    component_schema: if name == "Transform" {
                        &schema["properties"]["transform"]
                    } else {
                        &schema["properties"]["components"]["properties"][name]
                    },
                });
                section(ui, name, |ui| {
                    value_editor(ui, value, &mut Vec::new(), target.as_ref(), commands);
                });
            }
        }
    }
}

/// Which packages extend the definition and why, with the same add/remove operations the
/// protocol offers. Inferred ones are explained, not edited: they follow from components.
fn extensor_section(
    ui: &mut Ui,
    path: &str,
    extensors: &Extensors,
    playing: bool,
    commands: &mut Vec<Command>,
) {
    let add = |extensor: &str| Command::AddExtensor {
        definition: path.into(),
        extensor: extensor.into(),
    };
    let remove = |extensor: &str| Command::RemoveExtensor {
        definition: path.into(),
        extensor: extensor.into(),
    };
    section(ui, "Extensors", |ui| {
        if extensors.used.is_empty() && extensors.dropped.is_empty() {
            hint(ui, "None in use.");
        }
        for used in &extensors.used {
            ui.horizontal_wrapped(|ui| {
                let (why, editable) = match &used.reason {
                    ExtensorWhy::NamedBy(by) if by == path => ("named here".to_owned(), true),
                    ExtensorWhy::NamedBy(by) => (format!("named by {by}"), true),
                    ExtensorWhy::Owns(component) => (format!("from {component}"), false),
                    ExtensorWhy::RequiredBy(user) => (format!("needed by {user}"), false),
                };
                ui.label(RichText::new(&used.name).strong());
                ui.label(RichText::new(why).small().color(theme::MUTED));
                if !used.supplied.is_empty() {
                    hint(ui, &format!("supplies {}", used.supplied.join(", ")));
                }
                if editable
                    && !playing
                    && ui
                        .small_button("Remove")
                        .on_hover_text("Remove it and its own components; drops it if inherited")
                        .clicked()
                {
                    commands.push(remove(&used.name));
                }
            });
        }
        for dropped in &extensors.dropped {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(&dropped.name)
                        .strikethrough()
                        .color(theme::MUTED),
                );
                hint(ui, &format!("dropped by {}", dropped.by));
                if dropped.by == path
                    && !playing
                    && ui
                        .small_button("Restore")
                        .on_hover_text("Stop dropping it here")
                        .clicked()
                {
                    commands.push(add(&dropped.name));
                }
            });
        }
        if playing {
            return;
        }
        for suggested in &extensors.suggested {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .small_button(format!("+ {}", suggested.name))
                    .on_hover_text(&suggested.doc)
                    .clicked()
                {
                    commands.push(add(&suggested.name));
                }
                let mut why = format!("suggested: builds on {}", suggested.because.join(", "));
                if !suggested.supplies.is_empty() {
                    why += &format!("; supplies {}", suggested.supplies.join(", "));
                }
                hint(ui, &why);
            });
        }
        let others: Vec<_> = extensors
            .available
            .iter()
            .filter(|name| !extensors.suggested.iter().any(|s| &s.name == *name))
            .collect();
        if !others.is_empty() {
            ui.menu_button(RichText::new("Add extensor…").small(), |ui| {
                for name in others {
                    if ui.button(name).clicked() {
                        commands.push(add(name));
                        ui.close();
                    }
                }
            });
        }
    });
}

/// Where a component's fields are written, and what that source already sets.
struct FieldTarget<'a> {
    file: String,
    path: Vec<Field>,
    authored: Option<&'a Value>,
    owner: String,
    schema: &'a Value,
    component_schema: &'a Value,
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
    value_editor_at(ui, value, value, field, target, commands);
}

fn value_editor_at(
    ui: &mut Ui,
    root_value: &Value,
    value: &Value,
    field: &mut Vec<Field>,
    target: Option<&FieldTarget>,
    commands: &mut Vec<Command>,
) {
    use struction_editor::fields::{initial_value, schema_at, shape};
    let schema = target.and_then(|t| schema_at(t.schema, t.component_schema, root_value, field));
    let fixed_vector = value.as_array().is_some_and(|a| is_vector(a))
        && schema.is_none_or(|s| s.get("maxItems").is_some());
    match value {
        Value::Object(members) => {
            for (key, member) in members {
                field.push(Field::Key(key.clone()));
                if member.is_object() || member.is_array() {
                    ui.label(RichText::new(key).color(theme::MUTED));
                    ui.indent(key, |ui| {
                        value_editor_at(ui, root_value, member, field, target, commands)
                    });
                } else {
                    leaf_row(ui, key, member, field, target, commands);
                }
                field.pop();
            }
            if members.is_empty() {
                hint(ui, "No fields.");
            }
            if let (Some(t), Some(s)) = (target, schema) {
                ui.menu_button("Add field…", |ui| {
                    let mut any = false;
                    if let Some(props) = s.get("properties").and_then(Value::as_object) {
                        for (key, prop) in props {
                            let mut child = field.clone();
                            child.push(Field::Key(key.clone()));
                            if t.is_set(&child) {
                                continue;
                            }
                            any = true;
                            ui.menu_button(key, |ui| {
                                let default = members
                                    .get(key)
                                    .cloned()
                                    .or_else(|| initial_value(t.schema, prop));
                                if let Some(value) = addition_value(ui, key, default) {
                                    commands.push(Command::AddField {
                                        file: t.file.clone(),
                                        path: t.full_path(field),
                                        key: key.clone(),
                                        value,
                                    });
                                    ui.close();
                                }
                            });
                        }
                    }
                    if let Some(prop) = s.get("additionalProperties").filter(|v| v.is_object()) {
                        any = true;
                        let id = ui.id().with(("new-key", format!("{field:?}")));
                        let mut key = ui
                            .data_mut(|d| d.get_temp::<String>(id))
                            .unwrap_or_default();
                        ui.add(TextEdit::singleline(&mut key).hint_text("Field name"));
                        ui.data_mut(|d| d.insert_temp(id, key.clone()));
                        ui.add_enabled_ui(!key.is_empty() && !members.contains_key(&key), |ui| {
                            if let Some(value) =
                                addition_value(ui, "map", initial_value(t.schema, prop))
                            {
                                commands.push(Command::AddField {
                                    file: t.file.clone(),
                                    path: t.full_path(field),
                                    key,
                                    value,
                                });
                                ui.close();
                            }
                        });
                    }
                    if !any {
                        hint(ui, "All schema fields are already authored.");
                    }
                });
            }
        }
        Value::Array(items) if !fixed_vector => {
            for (index, item) in items.iter().enumerate() {
                field.push(Field::Index(index));
                if item.is_object() || item.is_array() {
                    ui.label(format!("Entry {index}"));
                    ui.indent(index, |ui| {
                        value_editor_at(ui, root_value, item, field, target, commands)
                    });
                } else {
                    leaf_row(ui, &index.to_string(), item, field, target, commands);
                }
                field.pop();
            }
            if let (Some(t), Some(s)) = (target, schema)
                && !s
                    .get("maxItems")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| items.len() >= n as usize)
                && let Some(item_schema) = s.get("items")
            {
                ui.menu_button("Add entry…", |ui| {
                    let item = shape(t.schema, item_schema, None);
                    if let Some(choices) = item.get("enum").and_then(Value::as_array) {
                        for choice in choices {
                            if ui.button(choice.as_str().unwrap_or("value")).clicked() {
                                commands.push(Command::AddEntry {
                                    file: t.file.clone(),
                                    path: t.full_path(field),
                                    value: choice.clone(),
                                });
                                ui.close();
                            }
                        }
                    } else if let Some(value) =
                        addition_value(ui, "entry", initial_value(t.schema, item_schema))
                    {
                        commands.push(Command::AddEntry {
                            file: t.file.clone(),
                            path: t.full_path(field),
                            value,
                        });
                        ui.close();
                    }
                });
            }
        }
        leaf => leaf_row(ui, "value", leaf, field, target, commands),
    }
}

/// Keep an editable draft until Add, so required references and timings can be supplied
/// before validation instead of forcing an invalid placeholder into the project.
fn addition_value(ui: &mut Ui, salt: &str, default: Option<Value>) -> Option<Value> {
    let id = ui.id().with(("addition", salt));
    let mut draft = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| {
            default.map_or(String::new(), |v| {
                serde_json::to_string_pretty(&v).unwrap_or_default()
            })
        });
    ui.label("Initial value (JSON)");
    ui.add(
        TextEdit::multiline(&mut draft)
            .desired_width(240.0)
            .desired_rows(3),
    );
    ui.data_mut(|d| d.insert_temp(id, draft.clone()));
    let parsed = serde_json::from_str::<Value>(&draft);
    if ui
        .add_enabled(parsed.is_ok(), egui::Button::new("Add"))
        .clicked()
    {
        ui.data_mut(|d| d.remove::<String>(id));
        parsed.ok()
    } else {
        if let Err(error) = parsed {
            hint(ui, &error.to_string());
        }
        None
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
                .filter_map(Value::as_f64)
                .map(|v| v as f32)
                .collect();
            if let Ok(vector) = <[f32; 3]>::try_from(vector.as_slice()) {
                return vector_fields(ui, Vec3::from(vector), true, 0.05, width)
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
                    if editor.diagnostics.iter().any(|d| {
                        d.file == diagnostic.file
                            && d.line == diagnostic.line
                            && d.column == diagnostic.column
                            && d.message == diagnostic.message
                    }) {
                        continue;
                    }
                    diagnostic_row(ui, diagnostic, warn, editor, commands);
                }
            });
        }
        if editor.project.is_some() && count == 0 && editor.rejection.is_none() {
            if editor.definitions.is_empty() {
                ui.label(RichText::new("No definitions found. Open the data directory containing entity.jsonc definitions and scenes/.").color(ui.visuals().warn_fg_color));
            } else {
                ui.label(RichText::new("All sources are valid.").color(theme::WARD));
            }
        }
        let error = ui.visuals().error_fg_color;
        for diagnostic in &editor.diagnostics {
            diagnostic_row(ui, diagnostic, error, editor, commands);
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

fn viewport_overlay(
    ctx: &egui::Context,
    free: egui::Rect,
    editor: &Editor,
    control: &crate::play_view::PlayControl,
    guides: &mut crate::spatial_guides::GuideSettings,
    camera: &Camera,
    transform: &GlobalTransform,
) {
    egui::Area::new("viewport_help".into()).fixed_pos(free.min + egui::vec2(10.0,8.0)).show(ctx, |ui| {
        Frame::new().fill(theme::BASE.gamma_multiply(0.94)).stroke(egui::Stroke::new(1.0,theme::BORDER)).inner_margin(8).show(ui, |ui| {
            ui.set_max_width((free.width()-40.0).max(100.0));
            if editor.playing() {
                let hint = if crate::play_view::camera_transform(editor).is_none() { "No PlayerControlled entity in this scene" }
                    else if control.captured { "WASD · mouse look · Space jump · Shift roll · F / click attack · V view · Esc release" }
                    else { "Click the view to control the player · Esc releases the mouse" };
                ui.label(RichText::new(hint).small().color(theme::ACTOR));
            } else {
                ui.checkbox(&mut guides.visible,"Spatial guides");
                ui.label(RichText::new("Right-drag orbit · middle-drag pan · F focus").small().color(theme::MUTED));
            }
        });
    });
    if !guides.visible || editor.playing() {
        return;
    }
    let Some(world) = crate::scene_view::source_world(editor) else {
        return;
    };
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            "guide_labels".into(),
        ))
        .with_clip_rect(free);
    for entry in editor.entities.iter().filter(|e| !e.disabled) {
        let Some(position) = entry.position else {
            continue;
        };
        let Ok(point) = camera.world_to_viewport(transform, position) else {
            continue;
        };
        let at = egui::pos2(point.x, point.y);
        if !free.contains(at) {
            continue;
        }
        if let Some(zone) = world.get::<struction_physics::CameraZone>(entry.entity) {
            let color = theme::ACTOR;
            let rect = egui::Rect::from_center_size(at, egui::vec2(20.0, 14.0));
            painter.rect(
                rect,
                2.0,
                theme::BASE,
                egui::Stroke::new(1.5, color),
                egui::StrokeKind::Inside,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    at + egui::vec2(10.0, -3.0),
                    at + egui::vec2(17.0, -7.0),
                    at + egui::vec2(17.0, 7.0),
                    at + egui::vec2(10.0, 3.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
            painter.text(
                at + egui::vec2(20.0, 0.0),
                egui::Align2::LEFT_CENTER,
                crate::spatial_guides::camera_label(zone),
                egui::FontId::proportional(11.0),
                color,
            );
        } else if let Some(field) = world.get::<struction_gravity::GravityField>(entry.entity) {
            let text = if matches!(field.volume, struction_gravity::GravityVolume::Infinite) {
                "↓ Gravity · infinite"
            } else {
                "↓ Gravity"
            };
            painter.text(
                at,
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(11.0),
                theme::DEFINITION,
            );
        }
    }
}
