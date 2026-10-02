//! The mesh tool window's panels: workspace tabs, the model summary and the workspace's controls
//! on the left, the 3D preview with its overlay switches in the middle (and the pose timeline
//! under it), and one apply bar at the bottom. Edits only change drafts; Apply writes them all,
//! the recipe through the preparation session and poses as a project edit, and Undo walks
//! them back in order.
use bevy::prelude::*;
use bevy_egui::{
    EguiContext, PrimaryEguiContext,
    egui::{
        self, Align, Color32, Frame, Key, KeyboardShortcut, Layout, Margin, Modifiers, RichText,
        Sense, Slider, Ui, UiBuilder,
    },
};
use struction_assets::{CollisionPreset, LodPreset, recipe::MAX_LOD_LEVELS};

use crate::theme;
use crate::tools::{
    After, Mode, Ready, Request, ToolEdit, ToolState, Toolbox, Workspace, finish_close,
    open_in_label, place_view,
};

pub fn tool_ui(
    mut toolbox: ResMut<Toolbox>,
    mut editor: NonSendMut<crate::state::Editor>,
    mut contexts: Query<&mut EguiContext, Without<PrimaryEguiContext>>,
    mut cameras: Query<&mut Camera>,
    windows: Query<&Window>,
) -> Result {
    let Ok(mut context) = contexts.single_mut() else {
        return Ok(());
    };
    let ctx = context.get_mut().clone();
    let open_error = toolbox.open_error.clone();
    let Some(tool) = &mut toolbox.tool else {
        return Ok(());
    };
    if !tool.themed {
        theme::apply(&ctx);
        tool.themed = true;
        return Ok(());
    }
    let mut requests = Vec::new();
    let mut answer = None;
    let mut close_clicked = false;

    if let Some(ready) = tool.ready_mut() {
        collect_applied(ready);
    }
    if !ctx.text_edit_focused() && tool.closing.is_none() {
        let workspace = tool.mode.workspace();
        let switch = ctx.input_mut(|input| {
            [Key::Num1, Key::Num2, Key::Num3]
                .into_iter()
                .zip(Workspace::ALL)
                .find(|(key, _)| input.consume_key(Modifiers::NONE, *key))
                .map(|(_, workspace)| workspace)
        });
        if let Some(ready) = tool.ready_mut() {
            let shortcut = |modifiers, key| {
                ctx.input_mut(|input| {
                    input.consume_shortcut(&KeyboardShortcut::new(modifiers, key))
                })
            };
            let redo = shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
            if redo {
                redo_tool(ready, &mut editor);
            } else if shortcut(Modifiers::COMMAND, Key::Z) {
                undo_tool(ready, &mut editor);
            }
            if shortcut(Modifiers::COMMAND, Key::Enter) {
                apply_all(ready, &mut editor);
            }
            if shortcut(Modifiers::NONE, Key::Escape) {
                revert_all(ready, &editor);
            }
            if workspace == Workspace::Poses {
                crate::pose_tool::shortcuts(&ctx, &mut ready.pose, &editor);
            }
        }
        if let Some(next) = switch {
            tool.mode = next.mode();
        }
    }

    let workspace = tool.mode.workspace();
    if let Some(ready) = tool.ready_mut() {
        ready.switch_view(workspace);
    }

    let mut root = Ui::new(
        ctx.clone(),
        "mesh_tool".into(),
        UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    let panel = Frame::new()
        .fill(theme::PANEL)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .inner_margin(Margin::same(12));
    let bottom = egui::Panel::bottom("tool_bar")
        .frame(
            Frame::new()
                .fill(theme::BASE)
                .inner_margin(Margin::symmetric(12, 8)),
        )
        .show(&mut root, |ui| {
            let (asset, source) = (tool.asset.clone(), tool.source.clone());
            bottom_bar(
                ui,
                tool.ready_mut(),
                &mut editor,
                &mut requests,
                &mut close_clicked,
                (asset, source),
            );
        })
        .response
        .rect
        .height();
    let left =
        egui::Panel::left("tool_panel")
            .resizable(true)
            .default_size(360.0)
            .size_range(280.0..=560.0)
            .frame(panel)
            .show(&mut root, |ui| {
                ui.label(RichText::new(tool.file_name()).heading());
                ui.label(RichText::new(&tool.asset).monospace().small().color(theme::MUTED));
                ui.add_space(8.0);
                let current = tool.mode.workspace();
                ui.horizontal(|ui| {
                    for (index, workspace) in Workspace::ALL.into_iter().enumerate() {
                        if ui
                            .selectable_label(current == workspace, workspace.label())
                            .on_hover_text(format!("{}", index + 1))
                            .clicked()
                            && workspace != current
                        {
                            tool.mode = workspace.mode();
                        }
                    }
                });
                if let Some(ready) = tool.ready_mut() {
                    summary(ui, ready);
                }
                ui.separator();
                let workspace = tool.mode.workspace();
                let asset = tool.asset.clone();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(error) = &open_error {
                        error_text(ui, error);
                    }
                    match &mut tool.state {
                        ToolState::Opening(_) => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Importing…");
                            });
                            hint(ui, "Blender runs in the background for .blend sources.");
                        }
                        ToolState::Failed(error) => {
                            error_text(ui, error);
                            hint(
                                ui,
                                "Fix the source, or choose Blender in Programs… on the main window, then reopen it.",
                            );
                        }
                        ToolState::Ready(ready) => match workspace {
                            Workspace::Prepare => {
                                egui::CollapsingHeader::new(theme::section("LODs"))
                                    .default_open(true)
                                    .show(ui, |ui| lods(ui, ready));
                                egui::CollapsingHeader::new(theme::section("Collision"))
                                    .default_open(true)
                                    .show(ui, |ui| collision(ui, ready));
                            }
                            Workspace::Surface => {
                                mesh_choice(ui, ready);
                                hint(ui, "Or click a part in the view.");
                                egui::CollapsingHeader::new(theme::section("UVs"))
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        uv_view(ui, ready);
                                        uv_panel(ui, ready, &asset, &mut requests);
                                    });
                                egui::CollapsingHeader::new(theme::section("Material"))
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        material_panel(ui, ready, &asset, &mut requests)
                                    });
                            }
                            Workspace::Poses => crate::pose_tool::panel(
                                ui,
                                &mut ready.pose,
                                &mut editor,
                                &asset,
                            ),
                        },
                    }
                    if let Some(ready) = tool.ready() {
                        ui.add_space(8.0);
                        if let Some(error) =
                            ready.error.as_ref().or(ready.preview_error.as_ref())
                        {
                            error_text(ui, error);
                        } else if let Some(status) = &ready.status {
                            hint(ui, status);
                        }
                    }
                });
                ui.allocate_rect(ui.available_rect_before_wrap(), Sense::hover());
            })
            .response
            .rect
            .width();
    let mode = tool.mode;
    let timeline = match tool.ready_mut() {
        Some(ready) if mode == Mode::Poses => egui::Panel::bottom("timeline")
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(&mut root, |ui| {
                crate::pose_tool::timeline(ui, &mut ready.pose)
            })
            .response
            .rect
            .height(),
        _ => 0.0,
    };

    let full = ctx.viewport_rect();
    let view = Rect::new(left, 0.0, full.max.x, full.max.y - bottom - timeline);
    let (camera, window) = tool.view_entities();
    if let (Ok(mut camera), Ok(window)) = (cameras.get_mut(camera), windows.get(window)) {
        tool.view = Some(place_view(
            &mut camera,
            window,
            view,
            ctx.pixels_per_point(),
        ));
    }
    caption(&ctx, tool.mode, tool.ready(), egui::pos2(left + 12.0, 10.0));
    let toggles = tool.ready_mut().map(|ready| {
        let workspace = mode.workspace();
        egui::Area::new("view_toggles".into())
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(egui::pos2(left + 12.0, view.max.y - 8.0))
            .constrain_to(egui::Rect::from_min_max(
                egui::pos2(view.min.x, view.min.y),
                egui::pos2(view.max.x, view.max.y),
            ))
            .show(&ctx, |ui| {
                ui.set_max_width(view.width() - 24.0);
                view_toggles(ui, ready, workspace)
            })
            .response
            .rect
    });

    if close_clicked {
        if tool.is_dirty() {
            tool.closing = Some(After::Close);
        } else {
            finish_close(&mut toolbox, After::Close);
            toolbox.requests.extend(requests);
            return Ok(());
        }
    }
    if let Some(after) = tool.closing.clone() {
        let name = tool.file_name().to_owned();
        let modal = egui::Modal::new(egui::Id::new("close_mesh_tool")).show(&ctx, |ui| {
            ui.set_width(340.0);
            ui.label(RichText::new(format!("Apply your changes to {name}?")).heading());
            ui.label(RichText::new("They are only a preview until applied.").color(theme::MUTED));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    answer = Some(Some(true));
                }
                if ui.button("Discard").clicked() {
                    answer = Some(Some(false));
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(None);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(None);
        }
        match answer {
            Some(Some(apply)) => {
                let applied = !apply
                    || tool.ready_mut().is_none_or(|ready| {
                        apply_all(ready, &mut editor);
                        ready.error.is_none() && !ready.is_dirty() && !ready.pose.dirty()
                    });
                tool.closing = None;
                if applied {
                    finish_close(&mut toolbox, after);
                }
            }
            Some(None) => tool.closing = None,
            None => {}
        }
    }
    if let Some(tool) = &mut toolbox.tool {
        let pointer = ctx.pointer_hover_pos();
        let over_toggles = toggles
            .zip(pointer)
            .is_some_and(|(rect, pointer)| rect.contains(pointer));
        tool.pointer_over_ui = tool.closing.is_some() || over_toggles;
    }
    toolbox.requests.extend(requests);
    Ok(())
}

/// Overlay switches over the view, each available in any workspace.
fn view_toggles(ui: &mut Ui, ready: &mut Ready, workspace: Workspace) {
    Frame::new()
        .fill(theme::PANEL.gamma_multiply(0.92))
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(6)
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal_wrapped(|ui| {
                let small = |text: &str| RichText::new(text).small();
                let view = &mut ready.view;
                ui.label(small("Show").color(theme::MUTED));
                ui.toggle_value(&mut view.wireframe, small("Wireframe"));
                ui.toggle_value(
                    &mut view.hull,
                    small("Hull").color(Color32::from_rgb(0x5a, 0xc7, 0xf2)),
                );
                ui.toggle_value(
                    &mut view.trimesh,
                    small("Trimesh").color(Color32::from_rgb(0x7c, 0xd9, 0x73)),
                );
                ui.toggle_value(&mut view.parts, small("Parts"))
                    .on_hover_text("Convex parts of the collision decomposition");
                if workspace != Workspace::Poses {
                    ui.toggle_value(&mut view.compare, small("LOD 0"))
                        .on_hover_text("Compare LOD 0 with the level chosen under LODs");
                } else {
                    ui.toggle_value(&mut ready.pose.ghosts, small("Ghosts"));
                }
            });
        });
}

/// The model at a glance, kept above every workspace; details fold out.
fn summary(ui: &mut Ui, ready: &mut Ready) {
    let Some(preview) = &ready.preview else {
        return;
    };
    let triangles: usize = preview.meshes.iter().map(|m| m.lods[0].triangles).sum();
    egui::CollapsingHeader::new(
        RichText::new(format!(
            "{} parts · {} triangles · {} materials",
            preview.meshes.len(),
            count(triangles),
            preview.bundle.materials.len()
        ))
        .small()
        .color(theme::MUTED),
    )
    .id_salt("model_summary")
    .show(ui, |ui| inspect(ui, ready));
}

/// Project edits the pose workspace made join the tool's undo order.
fn collect_applied(ready: &mut Ready) {
    for label in std::mem::take(&mut ready.pose.applied) {
        ready.order.push(ToolEdit::Project(label));
        ready.undone.clear();
    }
}

/// Walks back the tool's last applied edit, whichever kind it was. A project edit is only
/// undone while it is still the project's latest.
fn undo_tool(ready: &mut Ready, editor: &mut crate::state::Editor) {
    let Some(edit) = ready.order.last().cloned() else {
        return;
    };
    match &edit {
        ToolEdit::Recipe => ready.undo(),
        ToolEdit::Project(label) => {
            if project_undo(editor) != Some(label.clone()) {
                ready.error = Some(format!(
                    "The project's last edit is not \"{label}\"; undo it from the main window"
                ));
                return;
            }
            editor.apply(crate::state::Command::Undo);
            ready.pose.load(editor);
        }
    }
    ready.order.pop();
    ready.undone.push(edit);
}

fn redo_tool(ready: &mut Ready, editor: &mut crate::state::Editor) {
    let Some(edit) = ready.undone.last().cloned() else {
        return;
    };
    match &edit {
        ToolEdit::Recipe => ready.redo(),
        ToolEdit::Project(label) => {
            if project_redo(editor) != Some(label.clone()) {
                ready.error = Some(format!(
                    "The project can no longer redo \"{label}\"; it changed since"
                ));
                return;
            }
            editor.apply(crate::state::Command::Redo);
            ready.pose.load(editor);
        }
    }
    ready.undone.pop();
    ready.order.push(edit);
}

fn project_undo(editor: &crate::state::Editor) -> Option<String> {
    let project = editor.project.as_ref()?;
    project.session().history().undo_label().map(str::to_owned)
}

fn project_redo(editor: &crate::state::Editor) -> Option<String> {
    let project = editor.project.as_ref()?;
    project.session().history().redo_label().map(str::to_owned)
}

/// What Apply would write, by what it changes.
fn pending(ready: &Ready) -> Vec<String> {
    let mut pending: Vec<String> = Vec::new();
    let recipe = ready.pending_recipe();
    if !recipe.is_empty() {
        pending.push(format!("{} settings", recipe.join(", ")));
    }
    if ready.pose.dirty() {
        pending.push(format!("poses of {}", ready.pose.definition));
    }
    pending
}

/// Applies every pending change: recipe settings once their preview is ready, then pose
/// targets.
fn apply_all(ready: &mut Ready, editor: &mut crate::state::Editor) {
    let recipe = ready.pending_recipe();
    if !recipe.is_empty() && ready.is_current() {
        let label = format!("{} settings", recipe.join(", "));
        ready.apply(&label);
    }
    if ready.pose.dirty()
        && let Err(error) = ready.pose.apply(editor)
    {
        ready.error = Some(error);
    }
    collect_applied(ready);
}

fn revert_all(ready: &mut Ready, editor: &crate::state::Editor) {
    ready.revert();
    if ready.pose.dirty() {
        ready.pose.load(editor);
    }
}

fn bottom_bar(
    ui: &mut Ui,
    ready: Option<&mut Ready>,
    editor: &mut crate::state::Editor,
    requests: &mut Vec<Request>,
    close_clicked: &mut bool,
    (asset, source): (String, std::path::PathBuf),
) {
    ui.horizontal(|ui| {
        if let Some(ready) = ready {
            let undo = ready.order.last().map(edit_label);
            let redo = ready.undone.last().map(edit_label);
            if ui
                .add_enabled(undo.is_some(), egui::Button::new("Undo"))
                .on_hover_text(undo.map_or("Nothing to undo".into(), |l| format!("Undo {l} (Ctrl+Z)")))
                .clicked()
            {
                undo_tool(ready, editor);
            }
            if ui
                .add_enabled(redo.is_some(), egui::Button::new("Redo"))
                .on_hover_text(
                    redo.map_or("Nothing to redo".into(), |l| format!("Redo {l} (Ctrl+Shift+Z)")),
                )
                .clicked()
            {
                redo_tool(ready, editor);
            }
            ui.separator();
            let pending = pending(ready);
            let waiting = !ready.pending_recipe().is_empty() && !ready.is_current();
            let checked = ready.pose.check(editor);
            if pending.is_empty() {
                hint(ui, "Everything is applied.");
            } else {
                ui.colored_label(theme::ACCENT, format!("Not applied: {}", pending.join(" · ")));
                if ready.is_previewing() {
                    ui.spinner();
                }
            }
            if ui
                .add_enabled(
                    !pending.is_empty() && !waiting && (!ready.pose.dirty() || checked.is_ok()),
                    egui::Button::new("Apply"),
                )
                .on_hover_text("Write the recipe and pose targets (Ctrl+Enter)")
                .on_disabled_hover_text(if waiting {
                    "Waiting for the preview".to_owned()
                } else {
                    checked.err().unwrap_or("Nothing changed".into())
                })
                .clicked()
            {
                apply_all(ready, editor);
            }
            if ui
                .add_enabled(!pending.is_empty(), egui::Button::new("Revert"))
                .on_hover_text("Drop the changes not applied yet (Esc)")
                .clicked()
            {
                revert_all(ready, editor);
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Unapplied edits ask first; see `tool_ui`.
            *close_clicked = ui.button("Close").clicked();
            let label = open_in_label(&source);
            if ui
                .button(label)
                .on_hover_text("Edit the source in its full application; saves are picked up when this window regains focus")
                .clicked()
            {
                requests.push(Request::OpenExternally(asset));
            }
        });
    });
}

fn edit_label(edit: &ToolEdit) -> String {
    match edit {
        ToolEdit::Recipe => "the recipe change".into(),
        ToolEdit::Project(label) => label.clone(),
    }
}

fn inspect(ui: &mut Ui, ready: &mut Ready) {
    let session = &ready.session;
    let applied = session.applied();
    grid(ui, "recipe", |ui| {
        fact(ui, "Recipe");
        ui.label(if session.has_recipe() {
            "Saved beside the source"
        } else {
            "None: default settings"
        });
        ui.end_row();
        fact(ui, "LODs");
        ui.label(LodPreset::matching(&applied.lod).map_or("Custom", LodPreset::label));
        ui.end_row();
        fact(ui, "Collision");
        ui.label(
            CollisionPreset::matching(&applied.collision).map_or("Custom", CollisionPreset::label),
        );
        ui.end_row();
    });
    let Some(preview) = &ready.preview else {
        preparing(ui);
        return;
    };
    let triangles: usize = preview.meshes.iter().map(|m| m.lods[0].triangles).sum();
    let vertices: usize = preview.meshes.iter().map(|m| m.lods[0].vertices).sum();
    ui.add_space(6.0);
    ui.label(theme::section("Meshes"));
    grid(ui, "totals", |ui| {
        fact(ui, "Meshes");
        mono(ui, &preview.meshes.len().to_string());
        ui.end_row();
        fact(ui, "Triangles");
        mono(ui, &count(triangles));
        ui.end_row();
        fact(ui, "Vertices");
        mono(ui, &count(vertices));
        ui.end_row();
        fact(ui, "Materials");
        mono(ui, &preview.bundle.materials.len().to_string());
        ui.end_row();
    });
    let without_uvs = preview
        .bundle
        .meshes
        .iter()
        .filter(|m| m.lods[0].uvs.is_empty())
        .count();
    if without_uvs > 0 {
        warning(ui, &format!("{without_uvs} meshes have no UVs"));
    }
    ui.add_space(4.0);
    for (mesh, report) in preview.bundle.meshes.iter().zip(&preview.meshes) {
        let [x, y, z] = report.size;
        egui::CollapsingHeader::new(&report.name)
            .id_salt(("mesh", &report.name))
            .show(ui, |ui| {
                grid(ui, ("mesh_facts", &report.name), |ui| {
                    fact(ui, "Triangles");
                    mono(ui, &count(report.lods[0].triangles));
                    ui.end_row();
                    fact(ui, "Size");
                    mono(ui, &format!("{x:.2} × {y:.2} × {z:.2} m"));
                    ui.end_row();
                    fact(ui, "Material");
                    mono(ui, mesh.material.as_deref().unwrap_or("—"));
                    ui.end_row();
                });
            });
    }
}

fn lods(ui: &mut Ui, ready: &mut Ready) {
    let mut draft = ready.draft.clone();
    ui.label(theme::section("Preset"));
    let current = LodPreset::matching(&draft.lod);
    for preset in LodPreset::ALL {
        if preset_row(
            ui,
            current == Some(preset),
            preset.label(),
            preset.description(),
        ) {
            draft.lod = preset.settings();
        }
    }
    if current.is_none() {
        hint(ui, "Custom settings");
    }
    ui.add_space(6.0);
    ui.label(theme::section("Adjust"));
    let lod = &mut draft.lod;
    ui.add(Slider::new(&mut lod.levels, 0..=MAX_LOD_LEVELS).text("Levels"));
    ui.add_enabled(
        lod.levels > 0,
        Slider::new(&mut lod.reduction, 0.1..=0.9)
            .text("Keep per level")
            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
    )
    .on_hover_text("Triangles each level keeps from the previous one");
    ui.add_enabled(
        lod.levels > 0,
        Slider::new(&mut lod.max_error, 0.001..=0.5)
            .logarithmic(true)
            .text("Max error"),
    )
    .on_hover_text("Largest deviation allowed, relative to the mesh size; the simplifier stops before exceeding it");
    ready.set_draft(draft);

    let Some(preview) = ready.preview.as_ref().filter(|_| ready.is_current()) else {
        preparing(ui);
        return;
    };
    let levels = preview
        .meshes
        .iter()
        .map(|m| m.lods.len())
        .max()
        .unwrap_or(1);
    ui.add_space(6.0);
    ui.label(theme::section("Result"));
    grid(ui, "lod_totals", |ui| {
        fact(ui, "Level");
        fact(ui, "Triangles");
        fact(ui, "Kept");
        ui.end_row();
        let base: usize = preview.meshes.iter().map(|m| m.lods[0].triangles).sum();
        for level in 0..levels {
            let triangles: usize = preview
                .meshes
                .iter()
                .map(|m| {
                    m.lods
                        .get(level)
                        .unwrap_or(m.lods.last().expect("LOD 0"))
                        .triangles
                })
                .sum();
            mono(ui, &format!("LOD {level}"));
            mono(ui, &count(triangles));
            mono(
                ui,
                &format!("{:.0}%", 100.0 * triangles as f32 / base.max(1) as f32),
            );
            ui.end_row();
        }
    });
    if levels > 1 {
        let view = &mut ready.view;
        view.lod = view.lod.clamp(1, levels - 1);
        ui.checkbox(&mut view.compare, "Compare with LOD 0 in the view");
        ui.add_enabled(
            view.compare,
            Slider::new(&mut view.lod, 1..=levels - 1).text("Level"),
        );
    }
    notes(ui, preview.meshes.iter().flat_map(|m| &m.lod_notes));
}

fn collision(ui: &mut Ui, ready: &mut Ready) {
    let mut draft = ready.draft.clone();
    ui.label(theme::section("Preset"));
    let current = CollisionPreset::matching(&draft.collision);
    for preset in CollisionPreset::ALL {
        if preset_row(
            ui,
            current == Some(preset),
            preset.label(),
            preset.description(),
        ) {
            draft.collision = preset.settings();
        }
    }
    if current.is_none() {
        hint(ui, "Custom settings");
    }
    ui.add_space(6.0);
    ui.label(theme::section("Adjust"));
    let collision = &mut draft.collision;
    ui.add(
        Slider::new(&mut collision.trimesh_ratio, 0.02..=1.0)
            .text("Trimesh detail")
            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
    )
    .on_hover_text("Target triangle count of the static collision mesh, relative to the model");
    ui.add(
        Slider::new(&mut collision.trimesh_max_error, 0.001..=0.2)
            .logarithmic(true)
            .text("Trimesh max error"),
    );
    ui.checkbox(
        &mut collision.convex_decomposition,
        "Convex parts (concave dynamic bodies)",
    );
    ui.add_enabled_ui(collision.convex_decomposition, |ui| {
        ui.add(Slider::new(&mut collision.max_parts, 1..=64).text("Max parts"));
        ui.add(
            Slider::new(&mut collision.concavity, 0.001..=0.1)
                .logarithmic(true)
                .text("Concavity"),
        )
        .on_hover_text("How concave each part may stay; lower splits more finely");
    });
    ready.set_draft(draft);

    ui.add_space(6.0);
    ui.label(theme::section("Show"));
    let view = &mut ready.view;
    ui.horizontal(|ui| {
        ui.checkbox(&mut view.hull, legend("Hull", 0x5a, 0xc7, 0xf2));
        ui.checkbox(&mut view.trimesh, legend("Trimesh", 0x7c, 0xd9, 0x73));
        ui.checkbox(&mut view.parts, "Parts");
    });

    let Some(preview) = ready.preview.as_ref().filter(|_| ready.is_current()) else {
        preparing(ui);
        return;
    };
    ui.add_space(6.0);
    ui.label(theme::section("Result"));
    let hulls = preview.meshes.iter().filter(|m| m.hull.is_some()).count();
    let points: usize = preview
        .meshes
        .iter()
        .filter_map(|m| m.hull.map(|h| h.points))
        .sum();
    let trimesh: usize = preview.meshes.iter().map(|m| m.trimesh.triangles).sum();
    let parts: usize = preview.meshes.iter().map(|m| m.parts.len()).sum();
    grid(ui, "collision_totals", |ui| {
        fact(ui, "Convex hulls");
        mono(
            ui,
            &format!(
                "{hulls} of {} ({} points)",
                preview.meshes.len(),
                count(points)
            ),
        );
        ui.end_row();
        fact(ui, "Trimesh");
        mono(ui, &format!("{} triangles", count(trimesh)));
        ui.end_row();
        if preview.settings.collision.convex_decomposition {
            fact(ui, "Convex parts");
            mono(ui, &parts.to_string());
            ui.end_row();
        }
    });
    notes(ui, preview.meshes.iter().flat_map(|m| &m.collision_notes));
}

/// A short caption over the 3D view saying what it shows.
fn caption(ctx: &egui::Context, mode: Mode, ready: Option<&Ready>, at: egui::Pos2) {
    let Some(preview) = ready.and_then(|ready| ready.preview.as_ref()) else {
        return;
    };
    let comparing = ready.is_some_and(|ready| ready.view.compare);
    let text = match mode.workspace() {
        Workspace::Poses => {
            "Click a part to pick its joint · drag a ring to turn it · drag to orbit".into()
        }
        Workspace::Surface => "Click a part to select it · drag to orbit · wheel to zoom".into(),
        Workspace::Prepare if comparing => {
            let level = ready.map_or(1, |ready| ready.view.lod);
            let sum = |level: usize| -> usize {
                preview
                    .meshes
                    .iter()
                    .map(|m| {
                        m.lods
                            .get(level)
                            .unwrap_or(m.lods.last().expect("LOD 0"))
                            .triangles
                    })
                    .sum()
            };
            if preview.meshes.iter().all(|m| m.lods.len() < 2) {
                format!("LOD 0 · {} triangles (no further levels)", count(sum(0)))
            } else {
                format!(
                    "Left: LOD 0 · {} triangles    Right: LOD {level} · {} triangles",
                    count(sum(0)),
                    count(sum(level))
                )
            }
        }
        Workspace::Prepare => {
            "Click a part to select it · drag to orbit · middle-drag to pan".to_owned()
        }
    };
    egui::Area::new(egui::Id::new("tool_caption"))
        .fixed_pos(at)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::new()
                .fill(theme::BASE.gamma_multiply(0.85))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).small());
                });
        });
}

/// Distinct notes across meshes, with how many meshes each applies to.
fn notes<'a>(ui: &mut Ui, notes: impl Iterator<Item = &'a String>) {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for note in notes {
        match seen.iter_mut().find(|(text, _)| text == note) {
            Some((_, times)) => *times += 1,
            None => seen.push((note.clone(), 1)),
        }
    }
    for (note, times) in seen {
        let text = if times > 1 {
            format!("{note} ({times} meshes)")
        } else {
            note
        };
        warning(ui, &text);
    }
}

fn preset_row(ui: &mut Ui, selected: bool, label: &str, description: &str) -> bool {
    let clicked = ui.selectable_label(selected, label).clicked();
    ui.label(RichText::new(description).small().color(theme::MUTED));
    clicked
}

fn preparing(ui: &mut Ui) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spinner();
        hint(ui, "Preparing the preview…");
    });
}

fn legend(text: &str, r: u8, g: u8, b: u8) -> RichText {
    RichText::new(text).color(Color32::from_rgb(r, g, b))
}

fn grid(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, body: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, body);
}

fn count(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
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

fn warning(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(format!("⚠ {text}"))
            .small()
            .color(ui.visuals().warn_fg_color),
    );
}

fn error_text(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .small()
            .color(ui.visuals().error_fg_color),
    );
}

fn mesh_choice(ui: &mut Ui, ready: &mut Ready) {
    let Some(preview) = &ready.preview else {
        return;
    };
    ready.mesh_index = ready
        .mesh_index
        .min(preview.bundle.meshes.len().saturating_sub(1));
    let label = preview
        .bundle
        .meshes
        .get(ready.mesh_index)
        .map_or("No meshes", |mesh| mesh.name.as_str());
    egui::ComboBox::from_label("Part")
        .selected_text(label)
        .show_ui(ui, |ui| {
            for (index, mesh) in preview.bundle.meshes.iter().enumerate() {
                ui.selectable_value(&mut ready.mesh_index, index, &mesh.name);
            }
        });
}

fn handoff(
    ui: &mut Ui,
    ready: &Ready,
    asset: &str,
    workspace: struction_assets::blender::BlenderWorkspace,
    requests: &mut Vec<Request>,
) {
    if ui
        .button(format!("Open {} in Blender…", workspace.name()))
        .clicked()
    {
        let bundle = ready.preview.as_ref().map(|p| &p.bundle);
        let object = bundle
            .and_then(|b| {
                b.nodes
                    .iter()
                    .find(|node| node.meshes.contains(&(ready.mesh_index as u32)))
            })
            .map(|node| node.name.clone())
            .unwrap_or_default();
        let material = bundle
            .and_then(|b| b.meshes.get(ready.mesh_index))
            .and_then(|mesh| mesh.material.clone())
            .unwrap_or_default();
        requests.push(Request::BlenderWorkspace {
            asset: asset.into(),
            workspace,
            object,
            material,
        });
    }
    hint(
        ui,
        "Save in Blender, then Refresh here to reimport. For glTF, export back to the same source path.",
    );
}

fn uv_panel(ui: &mut Ui, ready: &mut Ready, asset: &str, requests: &mut Vec<Request>) {
    ui.label(theme::section("UV preparation"));
    let mut draft = ready.draft.clone();
    ui.checkbox(
        &mut draft.generate_uvs,
        "Generate missing UVs (Smart UV Project)",
    );
    ready.set_draft(draft);
    hint(
        ui,
        "Existing UVs are preserved. Apply stores the preparation recipe and compiled result; the source model stays editable.",
    );
    ui.separator();
    handoff(
        ui,
        ready,
        asset,
        struction_assets::blender::BlenderWorkspace::Uvs,
        requests,
    );
    hint(
        ui,
        "For seams, packing or replacing existing UVs, Blender opens the selected part in Edit Mode in UV Editing.",
    );
}

fn material_panel(ui: &mut Ui, ready: &mut Ready, asset: &str, requests: &mut Vec<Request>) {
    if let Some(preview) = &ready.preview
        && let Some(mesh) = preview.bundle.meshes.get(ready.mesh_index)
    {
        ui.label(theme::section("Material slot"));
        ui.label(mesh.material.as_deref().unwrap_or("No material assigned"));
        if let Some(material) = preview
            .bundle
            .materials
            .iter()
            .find(|m| Some(&m.name) == mesh.material.as_ref())
        {
            ui.label(format!(
                "Metallic {:.2} · roughness {:.2}",
                material.metallic, material.roughness
            ));
            let color = Color32::from_rgba_unmultiplied(
                (material.base_color[0] * 255.0) as u8,
                (material.base_color[1] * 255.0) as u8,
                (material.base_color[2] * 255.0) as u8,
                255,
            );
            ui.colored_label(color, "■ Base color");
        }
    }
    ui.separator();
    handoff(
        ui,
        ready,
        asset,
        struction_assets::blender::BlenderWorkspace::Materials,
        requests,
    );
    hint(
        ui,
        "Blender opens Shading with this object and material slot active. Edit its material or texture nodes there.",
    );
}

/// The selected part's UV layout over a checker tile, beside the part in the 3D view.
fn uv_view(ui: &mut Ui, ready: &Ready) {
    let size = ui.available_width().clamp(120.0, 340.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size + 18.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let square = egui::Rect::from_min_size(rect.min, egui::vec2(size, size));
    for x in 0..10 {
        for y in 0..10 {
            let cell = egui::Rect::from_min_size(
                square.min + egui::vec2(x as f32, y as f32) * size / 10.0,
                egui::vec2(size / 10.0, size / 10.0),
            );
            painter.rect_filled(
                cell,
                0.0,
                if (x + y) % 2 == 0 {
                    Color32::from_gray(38)
                } else {
                    Color32::from_gray(48)
                },
            );
        }
    }
    painter.rect_stroke(
        square,
        0.0,
        egui::Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
    let Some(mesh) = ready
        .preview
        .as_ref()
        .and_then(|p| p.bundle.meshes.get(ready.mesh_index))
        .and_then(|m| m.lods.first())
    else {
        return;
    };
    let point = |index: u32| {
        mesh.uvs
            .get(index as usize)
            .map(|uv| egui::pos2(square.min.x + uv[0] * size, square.max.y - uv[1] * size))
    };
    for triangle in mesh.indices.as_chunks::<3>().0 {
        if let (Some(a), Some(b), Some(c)) =
            (point(triangle[0]), point(triangle[1]), point(triangle[2]))
        {
            painter.add(egui::Shape::closed_line(
                vec![a, b, c],
                egui::Stroke::new(0.8, theme::WARD),
            ));
        }
    }
    if mesh.uvs.is_empty() {
        painter.text(
            square.center(),
            egui::Align2::CENTER_CENTER,
            "No UV coordinates",
            egui::FontId::proportional(16.0),
            theme::MUTED,
        );
    }
    painter.text(
        square.left_bottom() + egui::vec2(0.0, 6.0),
        egui::Align2::LEFT_TOP,
        "0,0 · UV tile 0–1",
        egui::FontId::monospace(11.0),
        theme::MUTED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Command, Editor};
    use struction_anim::base_pose::{BasePose, JointPose};
    use struction_assets::{Blender, PrepSession, read_recipe};

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(7), "7");
        assert_eq!(count(1234), "1,234");
        assert_eq!(count(1_234_567), "1,234,567");
    }

    /// A tetrahedron as a glTF source, Blender-free.
    fn tetrahedron(dir: &std::path::Path) -> std::path::PathBuf {
        let positions: [[f32; 3]; 4] = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let indices: [u16; 12] = [0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
        let mut bytes: Vec<u8> = positions
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        bytes.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
        std::fs::write(dir.join("tetra.bin"), &bytes).unwrap();
        let gltf = serde_json::json!({
            "asset": { "version": "2.0" },
            "buffers": [{ "uri": "tetra.bin", "byteLength": bytes.len() }],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": 48 },
                { "buffer": 0, "byteOffset": 48, "byteLength": 24 }
            ],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
                  "min": [0, 0, 0], "max": [1, 1, 1] },
                { "bufferView": 1, "componentType": 5123, "count": 12, "type": "SCALAR" }
            ],
            "meshes": [{ "name": "tetra", "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1 }] }],
            "nodes": [{ "name": "tetra", "mesh": 0 }],
            "scenes": [{ "nodes": [0] }],
            "scene": 0
        });
        let source = dir.join("tetra.gltf");
        std::fs::write(&source, gltf.to_string()).unwrap();
        source
    }

    /// One Apply writes recipe and pose changes; Undo walks them back newest first, and leaves
    /// a project edit made elsewhere in the meantime alone.
    #[test]
    fn apply_and_undo_cover_recipe_and_pose_edits_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scenes")).unwrap();
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../playground/project/scenes/milestone1.jsonc"),
            dir.path().join("scenes/milestone1.jsonc"),
        )
        .unwrap();
        let mut editor = Editor::default();
        editor.apply(Command::Open(dir.path().into()));
        let source = tetrahedron(dir.path());
        let mut ready = Ready::new(PrepSession::open(&source, Blender::default()).unwrap());
        ready.pose.definition = "characters/player".into();
        ready.pose.rig_name = "humanoid".into();
        ready.pose.load(&editor);

        let mut draft = ready.draft.clone();
        draft.collision.trimesh_ratio = 0.5;
        ready.set_draft(draft.clone());
        ready.preview = Some(ready.session.preview(&draft).unwrap());
        ready.pose.targets.poses.insert(
            "aim".into(),
            BasePose {
                joints: vec![JointPose::rotation("head", Vec3::new(0.0, 30.0, 0.0))],
                ..Default::default()
            },
        );
        assert_eq!(
            pending(&ready),
            ["collision settings", "poses of characters/player"]
        );
        apply_all(&mut ready, &mut editor);
        assert!(pending(&ready).is_empty(), "{:?}", ready.error);
        assert_eq!(
            ready.order,
            [
                ToolEdit::Recipe,
                ToolEdit::Project("Pose targets for characters/player".into())
            ]
        );
        let player = dir.path().join("characters/player/entity.jsonc");
        let has_poses = || {
            std::fs::read_to_string(&player)
                .unwrap()
                .contains("PoseTargets")
        };
        assert!(has_poses());
        assert_eq!(read_recipe(&source).unwrap(), Some(draft));

        undo_tool(&mut ready, &mut editor);
        assert!(!has_poses());
        assert!(ready.pose.targets.poses.is_empty());
        undo_tool(&mut ready, &mut editor);
        assert_eq!(read_recipe(&source).unwrap(), None);
        redo_tool(&mut ready, &mut editor);
        redo_tool(&mut ready, &mut editor);
        assert!(has_poses() && read_recipe(&source).unwrap().is_some());

        // Another edit lands on the project: the tool's pose edit is no longer its latest.
        editor
            .project
            .as_mut()
            .unwrap()
            .edit(struction_editor::EditRequest::Set {
                file: "characters/player/entity.jsonc".into(),
                path: ["components", "Attack", "duration"]
                    .map(|key| struction_editor::Field::Key(key.into()))
                    .into(),
                value: serde_json::json!(0.7),
                label: "Elsewhere".into(),
                group: None,
                revision: None,
            })
            .unwrap();
        undo_tool(&mut ready, &mut editor);
        assert!(has_poses());
        assert!(
            ready
                .error
                .as_deref()
                .is_some_and(|e| e.contains("main window"))
        );
    }
}
