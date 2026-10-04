//! Schema-driven forms shared by existing values and new-entry drafts.
use bevy_egui::egui::{self, DragValue, RichText, TextEdit, Ui};
use serde_json::Value;
use struction_editor::{
    EditRequest, Field,
    fields::{initial_value, lookup, shape},
};

use crate::{state::Command, theme};

pub(crate) struct FieldTarget<'a> {
    pub file: String,
    pub path: Vec<Field>,
    pub authored: Option<&'a Value>,
    pub owner: String,
    pub schema: &'a Value,
    pub component_schema: &'a Value,
}
impl FieldTarget<'_> {
    fn path(&self, field: &[Field]) -> Vec<Field> {
        self.path.iter().chain(field).cloned().collect()
    }
    fn is_set(&self, field: &[Field]) -> bool {
        self.authored.and_then(|v| lookup(v, field)).is_some()
    }
    fn set(&self, field: &[Field], value: Value) -> Command {
        let path = self.path(field);
        Command::EditField {
            file: self.file.clone(),
            group: Some(format!("inspector:{}:{path:?}", self.file)),
            path,
            value,
        }
    }
    fn reset(&self, field: &[Field]) -> Command {
        Command::Edit(EditRequest::Remove {
            file: self.file.clone(),
            path: self.path(field),
            label: format!("Reset override on {}", self.owner),
            revision: None,
        })
    }
}

fn dereference<'a>(root: &'a Value, mut schema: &'a Value) -> &'a Value {
    for _ in 0..32 {
        let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
            break;
        };
        let Some(next) = reference.strip_prefix('#').and_then(|p| root.pointer(p)) else {
            break;
        };
        schema = next;
    }
    schema
}

fn choices(root: &Value, schema: &Value) -> Vec<Value> {
    fn gather(root: &Value, schema: &Value, depth: usize) -> Vec<Value> {
        if depth > 24 {
            return vec![];
        }
        let schema = dereference(root, schema);
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            return values.clone();
        }
        if let Some(value) = schema.get("const") {
            return vec![value.clone()];
        }
        if let Some(branches) = schema
            .get("oneOf")
            .or_else(|| schema.get("anyOf"))
            .and_then(Value::as_array)
        {
            return branches
                .iter()
                .flat_map(|b| gather(root, b, depth + 1))
                .collect();
        }
        if schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|r| r.len() == 1)
            && schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|p| p.len() == 1)
        {
            return initial_value(root, schema).into_iter().collect();
        }
        vec![]
    }
    gather(root, schema, 0)
}
fn choice_label(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            value
                .as_object()
                .filter(|o| o.len() == 1)
                .and_then(|o| o.keys().next().cloned())
        })
        .unwrap_or_else(|| value.to_string())
}
fn same_choice(a: &Value, b: &Value) -> bool {
    if a.is_object() && b.is_object() {
        choice_label(a) == choice_label(b)
    } else {
        a == b
    }
}
fn optional(root: &Value, schema: &Value) -> bool {
    let schema = dereference(root, schema);
    schema
        .get("anyOf")
        .and_then(Value::as_array)
        .is_some_and(|v| v.iter().any(|s| s["type"] == "null"))
}
fn fixed_array(schema: &Value) -> bool {
    schema.get("maxItems").is_some() && schema.get("minItems") == schema.get("maxItems")
}
fn child_schema<'a>(schema: &'a Value, key: &str) -> &'a Value {
    schema
        .get("properties")
        .and_then(|p| p.get(key))
        .or_else(|| schema.get("additionalProperties").filter(|p| p.is_object()))
        .unwrap_or(&Value::Null)
}
fn label(ui: &mut Ui, text: &str, schema: &Value) {
    let response = ui.label(RichText::new(text.replace('_', " ")).color(theme::MUTED));
    if let Some(doc) = schema.get("description").and_then(Value::as_str) {
        response.on_hover_text(doc);
    }
}

pub(crate) fn value_editor(
    ui: &mut Ui,
    value: &Value,
    field: &mut Vec<Field>,
    target: Option<&FieldTarget>,
    commands: &mut Vec<Command>,
) {
    if let Some(target) = target {
        ui.push_id((&target.file, format!("{:?}", target.path)), |ui| {
            live(
                ui,
                "",
                value,
                target.component_schema,
                field,
                target,
                commands,
            );
        });
        if ui.input(|i| i.pointer.any_released()) {
            commands.push(Command::EndGroup);
        }
    } else {
        ui.add_enabled_ui(false, |ui| {
            let mut value = value.clone();
            form(ui, &mut value, &Value::Null, &Value::Null, 0);
        });
    }
}

fn reset_button(ui: &mut Ui, field: &[Field], target: &FieldTarget, commands: &mut Vec<Command>) {
    if !target.is_set(field) {
        return;
    }
    // List elements are values, not sparse overrides. Removing their required fields would
    // make the whole entry invalid; reset the owning list or remove the entry instead.
    let nested = field.iter().any(|f| matches!(f, Field::Index(_)));
    if ui
        .add_enabled(!nested, egui::Button::new("Reset").small())
        .on_hover_text(if nested {
            "Reset the owning list to inherit it again, or remove this entry."
        } else {
            "Remove this field from this source; inherited/default values become visible if present."
        })
        .clicked()
    {
        commands.push(target.reset(field));
    }
}

fn live(
    ui: &mut Ui,
    name: &str,
    value: &Value,
    raw: &Value,
    field: &mut Vec<Field>,
    target: &FieldTarget,
    commands: &mut Vec<Command>,
) {
    let schema = shape(target.schema, raw, Some(value));
    let enum_values = choices(target.schema, raw);
    let scalar = !value.is_object() && !value.is_array();
    let vector = value
        .as_array()
        .is_some_and(|v| (2..=4).contains(&v.len()) && v.iter().all(Value::is_number))
        && fixed_array(schema);
    if scalar || vector {
        ui.horizontal_wrapped(|ui| {
            label(ui, name, schema);
            let mut draft = value.clone();
            let changed = form(ui, &mut draft, target.schema, raw, 0);
            if changed {
                commands.push(target.set(field, draft));
                if !ui.input(|i| i.pointer.primary_down()) {
                    commands.push(Command::EndGroup);
                }
            }
            reset_button(ui, field, target, commands);
        });
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if !name.is_empty() {
            label(ui, name, schema);
        }
        if let Some(items) = value.as_array() {
            ui.weak(format!("{} entries", items.len()));
        }
        if !enum_values.is_empty() {
            let mut draft = value.clone();
            if enum_picker(ui, &mut draft, &enum_values) {
                commands.push(target.set(field, draft));
                commands.push(Command::EndGroup);
            }
        }
        if !field.is_empty()
            && optional(target.schema, raw)
            && ui
                .small_button("Clear value")
                .on_hover_text("Set this optional value to None")
                .clicked()
        {
            commands.push(target.set(field, Value::Null));
            commands.push(Command::EndGroup);
        }
        reset_button(ui, field, target, commands);
    });
    ui.indent(("contents", name), |ui| match value {
        Value::Object(members) => {
            for (key, value) in members {
                field.push(Field::Key(key.clone()));
                ui.push_id(key, |ui| {
                    live(
                        ui,
                        key,
                        value,
                        child_schema(schema, key),
                        field,
                        target,
                        commands,
                    )
                });
                field.pop();
            }
            let absent: Vec<_> = schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter(|(k, _)| !members.contains_key(*k))
                .collect();
            if !absent.is_empty() {
                addition_menu(ui, "Add field…", |ui| {
                    for (key, prop) in absent {
                        ui.menu_button(key.replace('_', " "), |ui| {
                            if let Some(value) = addition(ui, target.schema, prop) {
                                commands.push(Command::AddField {
                                    file: target.file.clone(),
                                    path: target.path(field),
                                    key: key.clone(),
                                    value,
                                });
                                ui.close();
                            }
                        });
                    }
                });
            }
            if let Some(prop) = schema.get("additionalProperties").filter(|s| s.is_object()) {
                addition_menu(ui, "Add field…", |ui| {
                    let id = ui.id().with("key");
                    let mut key = ui
                        .data_mut(|d| d.get_temp::<String>(id))
                        .unwrap_or_default();
                    ui.add(TextEdit::singleline(&mut key).hint_text("New name"));
                    ui.data_mut(|d| d.insert_temp(id, key.clone()));
                    ui.add_enabled_ui(
                        !key.trim().is_empty() && !members.contains_key(&key),
                        |ui| {
                            if let Some(value) = addition(ui, target.schema, prop) {
                                commands.push(Command::AddField {
                                    file: target.file.clone(),
                                    path: target.path(field),
                                    key,
                                    value,
                                });
                                ui.close();
                            }
                        },
                    );
                });
            }
        }
        Value::Array(items) => {
            let removable = !fixed_array(schema)
                && items.len() > schema["minItems"].as_u64().unwrap_or(0) as usize;
            for (index, value) in items.iter().enumerate() {
                let item_schema = schema
                    .get("prefixItems")
                    .and_then(|p| p.get(index))
                    .or_else(|| schema.get("items"))
                    .unwrap_or(&Value::Null);
                ui.push_id(index, |ui| {
                    ui.horizontal(|ui| {
                        ui.weak(format!("{}", index + 1));
                        if removable && ui.small_button("Remove entry").clicked() {
                            commands.push(Command::RemoveEntry {
                                file: target.file.clone(),
                                path: target.path(field),
                                index,
                            });
                        }
                    });
                    field.push(Field::Index(index));
                    live(ui, "", value, item_schema, field, target, commands);
                    field.pop();
                });
            }
            if !fixed_array(schema) {
                ui.horizontal(|ui| {
                    let can_add = schema["maxItems"]
                        .as_u64()
                        .is_none_or(|max| items.len() < max as usize);
                    ui.add_enabled_ui(can_add, |ui| {
                        addition_menu(ui, "Add entry…", |ui| {
                            if let Some(value) = addition(ui, target.schema, &schema["items"]) {
                                commands.push(Command::AddEntry {
                                    file: target.file.clone(),
                                    path: target.path(field),
                                    value,
                                });
                                ui.close();
                            }
                        });
                    });
                    if !items.is_empty()
                        && schema["minItems"].as_u64().unwrap_or(0) == 0
                        && ui
                            .small_button("Clear list")
                            .on_hover_text("Use an empty list here; Reset restores inheritance.")
                            .clicked()
                    {
                        commands.push(target.set(field, Value::Array(vec![])));
                        commands.push(Command::EndGroup);
                    }
                });
            }
        }
        _ => {}
    });
}

fn enum_picker(ui: &mut Ui, value: &mut Value, choices: &[Value]) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt("choice")
        .selected_text(choice_label(value))
        .width(150.0)
        .show_ui(ui, |ui| {
            for choice in choices {
                if ui
                    .selectable_label(same_choice(value, choice), choice_label(choice))
                    .clicked()
                    && !same_choice(value, choice)
                {
                    *value = choice.clone();
                    changed = true;
                }
            }
        });
    changed
}

/// A temporary typed form. Its values only reach the project after Add is pressed.
fn addition(ui: &mut Ui, root: &Value, schema: &Value) -> Option<Value> {
    let templates = entry_templates(root, schema);
    if templates.is_empty() {
        return addition_form(ui, root, schema, initial_value(root, schema));
    }
    let mut added = None;
    for (name, value) in templates {
        if value.is_object() || value.is_array() {
            ui.menu_button(name, |ui| {
                added = addition_form(ui, root, schema, Some(value));
            });
        } else if ui.button(name).clicked() {
            added = Some(value);
        }
    }
    added
}

fn addition_menu(ui: &mut Ui, label: &str, body: impl FnOnce(&mut Ui)) {
    egui::containers::menu::MenuButton::new(label)
        .config(
            egui::containers::menu::MenuConfig::new()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
        )
        .ui(ui, body);
}

/// Offer the enum or a struct's action/state up front, rather than adding its first default.
fn entry_templates(root: &Value, schema: &Value) -> Vec<(String, Value)> {
    let variants = choices(root, schema);
    if !variants.is_empty() {
        return variants
            .into_iter()
            .map(|value| (choice_label(&value), value))
            .collect();
    }
    let schema = shape(root, schema, None);
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return vec![];
    };
    let Some((key, variants)) = ["action", "state", "condition", "kind"]
        .into_iter()
        .filter_map(|key| {
            properties
                .get(key)
                .map(|schema| (key, choices(root, schema)))
        })
        .find(|(_, variants)| !variants.is_empty())
    else {
        return vec![];
    };
    let Some(base) = initial_value(root, schema) else {
        return vec![];
    };
    variants
        .into_iter()
        .map(|variant| {
            let name = choice_label(&variant);
            let mut value = base.clone();
            value[key] = variant;
            (name, value)
        })
        .collect()
}

fn addition_form(
    ui: &mut Ui,
    root: &Value,
    schema: &Value,
    initial: Option<Value>,
) -> Option<Value> {
    let id = ui.id().with("new-value");
    let Some(mut value) = ui.data_mut(|d| d.get_temp::<Value>(id)).or(initial) else {
        ui.weak("No editable schema is registered for this field.");
        return None;
    };
    ui.set_min_width(230.0);
    form(ui, &mut value, root, schema, 0);
    ui.data_mut(|d| d.insert_temp(id, value.clone()));
    if ui.button("Add").clicked() {
        ui.data_mut(|d| d.remove::<Value>(id));
        Some(value)
    } else {
        None
    }
}

fn form(ui: &mut Ui, value: &mut Value, root: &Value, raw: &Value, depth: usize) -> bool {
    if depth > 20 {
        ui.weak("Nested value is too deep to display.");
        return false;
    }
    let mut changed = false;
    if optional(root, raw) {
        let mut enabled = !value.is_null();
        if ui.checkbox(&mut enabled, "Set value").changed() {
            *value = if enabled {
                initial_value(root, raw).unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            changed = true;
        }
        if !enabled {
            return changed;
        }
    }
    let choices = choices(root, raw);
    if !choices.is_empty() {
        changed |= enum_picker(ui, value, &choices);
        if !value.is_object() {
            return changed;
        }
    }
    let schema = shape(root, raw, Some(value));
    match value {
        Value::Object(members) => {
            for (key, value) in members.iter_mut() {
                ui.push_id(key, |ui| {
                    label(ui, key, child_schema(schema, key));
                    changed |= form(ui, value, root, child_schema(schema, key), depth + 1);
                });
            }
        }
        Value::Array(items) => {
            let mut remove = None;
            let removable = !fixed_array(schema)
                && items.len() > schema["minItems"].as_u64().unwrap_or(0) as usize;
            for (index, item) in items.iter_mut().enumerate() {
                ui.push_id(index, |ui| {
                    let s = schema
                        .get("prefixItems")
                        .and_then(|p| p.get(index))
                        .or_else(|| schema.get("items"))
                        .unwrap_or(&Value::Null);
                    ui.horizontal(|ui| {
                        ui.weak(if fixed_array(schema) && index < 4 {
                            ["X", "Y", "Z", "W"][index].to_owned()
                        } else {
                            (index + 1).to_string()
                        });
                        changed |= form(ui, item, root, s, depth + 1);
                        if removable && ui.small_button("×").on_hover_text("Remove entry").clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
            }
            if let Some(index) = remove {
                items.remove(index);
                changed = true;
            }
            if !fixed_array(schema)
                && schema["maxItems"]
                    .as_u64()
                    .is_none_or(|max| items.len() < max as usize)
                && ui.small_button("Add entry").clicked()
                && let Some(value) = initial_value(root, &schema["items"])
            {
                items.push(value);
                changed = true;
            }
        }
        Value::Bool(flag) => {
            changed |= ui.checkbox(flag, "").changed();
        }
        Value::Number(number) => {
            let min = schema["minimum"].as_f64();
            let max = schema["maximum"].as_f64();
            // The schema, not JSON's spelling (e.g. 0 versus 0.0), determines integer steps.
            let integer = schema["type"] == "integer";
            let mut n = number.as_f64().unwrap_or_default();
            let response = if let (Some(min), Some(max)) = (min, max) {
                let slider = egui::Slider::new(&mut n, min..=max);
                ui.add(if integer { slider.integer() } else { slider })
            } else {
                ui.add(
                    DragValue::new(&mut n)
                        .speed(if integer { 1.0 } else { 0.01 })
                        .range(min.unwrap_or(f64::NEG_INFINITY)..=max.unwrap_or(f64::INFINITY))
                        .max_decimals(if integer { 0 } else { 4 }),
                )
            };
            if response.changed() {
                *value = if integer {
                    Value::from(n as i64)
                } else {
                    Value::from(n)
                };
                changed = true;
            }
        }
        Value::String(text) if choices.is_empty() => {
            let id = ui.id().with("text");
            let mut draft = ui
                .data_mut(|d| d.get_temp::<String>(id))
                .unwrap_or_else(|| text.clone());
            let response = ui.add(TextEdit::singleline(&mut draft).desired_width(160.0));
            if response.has_focus() {
                ui.data_mut(|d| d.insert_temp(id, draft.clone()));
            }
            if response.lost_focus()
                || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                changed |= *text != draft;
                *text = draft;
                ui.data_mut(|d| d.remove::<String>(id));
            }
        }
        Value::Null => {
            ui.weak("None");
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entries_offer_every_state_and_cancel_action_before_configuring() {
        let schema = json!({
            "$defs": {
                "Action": {"enum": ["Jump", "Roll", "Attack"]},
                "State": {"enum": ["Grounded", "Airborne", "Swimming", "Rolling", "Attacking"]}
            },
            "type": "object",
            "properties": {
                "action": {"$ref": "#/$defs/Action"},
                "after": {"type": "number"}
            }
        });
        assert_eq!(
            entry_templates(&schema, &schema),
            vec![
                ("Jump".into(), json!({"action":"Jump", "after":0})),
                ("Roll".into(), json!({"action":"Roll", "after":0})),
                ("Attack".into(), json!({"action":"Attack", "after":0})),
            ]
        );
        let states = entry_templates(&schema, &json!({"$ref":"#/$defs/State"}));
        assert_eq!(states.len(), 5);
        assert_eq!(states[3], ("Rolling".into(), json!("Rolling")));
    }

    #[test]
    fn entry_menu_stays_open_while_configuring_and_adds_the_chosen_action() {
        let ctx = egui::Context::default();
        let schema = json!({"type":"object","properties":{
            "action":{"enum":["Jump","Roll","Attack"]}, "after":{"type":"number"}
        }});
        let mut added = None;
        let mut draw = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    addition_menu(ui, "Add entry…", |ui| {
                        if let Some(value) = addition(ui, &schema, &schema) {
                            added = Some(value);
                            ui.close();
                        }
                    });
                },
            );
            output.textures_delta.clear();
            output
        };
        let output = draw(vec![]);
        let mut click = |position| {
            draw(vec![egui::Event::PointerMoved(position)]);
            draw(vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]);
            draw(vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            draw(vec![])
        };
        let output = click(text_position(&output, "Add entry…"));
        text_position(&output, "Jump");
        text_position(&output, "Roll");
        let output = click(text_position(&output, "Attack"));
        let output = click(text_position(&output, "0.0"));
        text_position(&output, "after");
        click(text_position(&output, "Add"));
        assert_eq!(added, Some(json!({"action":"Attack","after":0})));
    }

    #[test]
    fn state_choices_follow_refs_and_tagged_variants_keep_their_payload() {
        let schema = json!({"$defs":{"State":{"oneOf":[{"enum":["Rolling","Attacking"]},{"type":"object","properties":{"Window":{"type":"object","properties":{"after":{"type":"number"}}}},"required":["Window"]}]}}});
        let values = choices(&schema, &json!({"$ref":"#/$defs/State"}));
        assert_eq!(
            values,
            [
                json!("Rolling"),
                json!("Attacking"),
                json!({"Window":{"after":0}})
            ]
        );
        assert!(same_choice(&values[2], &json!({"Window":{"after":0.8}})));
    }

    fn frame(
        ctx: &egui::Context,
        value: &mut Value,
        schema: &Value,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 400.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                form(ui, value, schema, schema, 0);
            },
        );
        output.textures_delta.clear();
        output
    }
    fn text_position(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
        fn find(shape: &egui::epaint::Shape, text: &str) -> Option<egui::Pos2> {
            match shape {
                egui::epaint::Shape::Text(s) if s.galley.text() == text => {
                    Some(s.pos + s.galley.size() * 0.5)
                }
                egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, text)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|s| find(&s.shape, text))
            .unwrap_or_else(|| panic!("Missing control {text}"))
    }
    fn click(
        ctx: &egui::Context,
        value: &mut Value,
        schema: &Value,
        pos: egui::Pos2,
    ) -> egui::FullOutput {
        frame(ctx, value, schema, vec![egui::Event::PointerMoved(pos)]);
        frame(
            ctx,
            value,
            schema,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            ctx,
            value,
            schema,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        frame(ctx, value, schema, vec![])
    }

    #[test]
    fn existing_state_is_changed_through_a_dropdown() {
        let ctx = egui::Context::default();
        let schema = json!({"enum":["Rolling","Attacking","Swimming"]});
        let mut value = json!("Rolling");
        let output = frame(&ctx, &mut value, &schema, vec![]);
        let output = click(&ctx, &mut value, &schema, text_position(&output, "Rolling"));
        click(
            &ctx,
            &mut value,
            &schema,
            text_position(&output, "Swimming"),
        );
        assert_eq!(value, "Swimming");
    }

    #[test]
    fn structured_addition_exposes_action_dropdown_and_timing_control() {
        let ctx = egui::Context::default();
        let schema = json!({"type":"object","properties":{"action":{"enum":["Jump","Roll","Attack"]},"after":{"type":"number"}}});
        let mut value = initial_value(&schema, &schema).unwrap();
        let output = frame(&ctx, &mut value, &schema, vec![]);
        text_position(&output, "after");
        let output = click(&ctx, &mut value, &schema, text_position(&output, "Jump"));
        click(&ctx, &mut value, &schema, text_position(&output, "Attack"));
        assert_eq!(value, json!({"action":"Attack","after":0}));
    }
}
