use serde_json::json;
use struction_data::edit::*;

const OGRE: &str = include_str!("fixtures/edit/ogre.jsonc");

fn path(p: &str) -> Vec<PathSegment> {
    parse_path(p)
}

#[test]
fn parse_paths() {
    assert_eq!(
        path("components.Health.max"),
        ["components", "Health", "max"].map(PathSegment::from)
    );
    assert_eq!(
        path("reactions[0].call"),
        [
            PathSegment::from("reactions"),
            PathSegment::from(0),
            PathSegment::from("call")
        ]
    );
}

#[test]
fn set_replaces_a_value_and_keeps_everything_else_byte_for_byte() {
    let edited = set_value(OGRE, &path("components.Health.max"), json!(75.5)).unwrap();
    assert_eq!(edited.edit.previous, Some(json!(60.0)));
    assert_eq!(edited.edit.next, Some(json!(75.5)));
    assert_eq!(
        edited.text,
        OGRE.replacen("\"max\": 60.0", "\"max\": 75.5", 1)
    );
    // Every comment survived.
    for comment in [
        "// A big dumb brute.",
        "// primordial base",
        "/* placement",
        "// Bigger than",
        "// hit points",
    ] {
        assert!(edited.text.contains(comment), "{comment}");
    }
}

#[test]
fn set_replaces_composite_values_and_array_elements() {
    let edited = set_value(OGRE, &path("components.Loot.items"), json!(["sword"])).unwrap();
    assert_eq!(edited.edit.previous, Some(json!(["club", "rock"])));
    assert!(edited.text.contains("\"items\": [\"sword\"]"));

    let edited = set_value(OGRE, &path("components.Loot.items[1]"), json!("pebble")).unwrap();
    assert_eq!(edited.edit.previous, Some(json!("rock")));
    assert!(
        edited.text.contains("[\"club\", \"pebble\"]"),
        "{}",
        edited.text
    );

    // Index == len appends.
    let edited = set_value(OGRE, &path("components.Loot.items[2]"), json!("bone")).unwrap();
    assert!(edited.text.contains("\"bone\""));
    assert_eq!(
        get_value(&edited.text, &path("components.Loot.items")).unwrap(),
        Some(json!(["club", "rock", "bone"]))
    );

    assert_eq!(
        set_value(OGRE, &path("components.Loot.items[9]"), json!(1)).unwrap_err(),
        EditError::IndexOutOfRange("components.Loot.items[9]".into())
    );
}

#[test]
fn set_creates_missing_fields_and_objects() {
    let edited = set_value(
        OGRE,
        &path("components.Flammable.ignition_temperature"),
        json!(300.0),
    )
    .unwrap();
    assert_eq!(edited.edit.previous, None);
    assert_eq!(
        get_value(&edited.text, &path("components.Flammable")).unwrap(),
        Some(json!({ "ignition_temperature": 300.0 }))
    );
    // Untouched parts are still verbatim.
    assert!(
        edited
            .text
            .contains("\"Health\": { \"current\": 60.0, \"max\": 60.0 }, // hit points")
    );
}

#[test]
fn new_top_level_sections_go_to_their_canonical_position() {
    let source =
        "{\n  \"descendsFrom\": \"Actor\",\n  // reactions come last\n  \"reactions\": []\n}\n";
    let edited = set_value(source, &path("components.Health.max"), json!(5)).unwrap();
    let keys = top_level_keys(&edited.text);
    assert_eq!(keys, ["descendsFrom", "components", "reactions"]);
    assert!(
        edited
            .text
            .contains("// reactions come last\n  \"reactions\"")
    );

    let edited = set_value(&edited.text, &path("transform.scale"), json!([2, 2, 2])).unwrap();
    assert_eq!(
        top_level_keys(&edited.text),
        ["descendsFrom", "transform", "components", "reactions"]
    );
    assert_eq!(check_canonical_order(&edited.text).unwrap(), None);

    let edited = set_value(&edited.text, &path("brain"), json!("ai/x")).unwrap();
    assert_eq!(
        top_level_keys(&edited.text),
        [
            "descendsFrom",
            "transform",
            "components",
            "reactions",
            "brain"
        ]
    );
}

fn top_level_keys(text: &str) -> Vec<String> {
    let value = get_value(text, &[]).unwrap().unwrap();
    value.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn remove_returns_the_previous_value() {
    let edited = remove_value(OGRE, &path("components.Loot.gold")).unwrap();
    assert_eq!(edited.edit.previous, Some(json!(12)));
    assert_eq!(edited.edit.next, None);
    assert_eq!(
        get_value(&edited.text, &path("components.Loot.gold")).unwrap(),
        None
    );
    assert_eq!(
        get_value(&edited.text, &path("components.Loot.items")).unwrap(),
        Some(json!(["club", "rock"]))
    );

    let edited = remove_value(OGRE, &path("components.Health")).unwrap();
    assert!(edited.text.contains("// hit points") || !edited.text.contains("\"Health\""));
    assert_eq!(
        get_value(&edited.text, &path("components.Health")).unwrap(),
        None
    );
    // The document is still valid JSONC and the neighbour intact.
    assert!(
        get_value(&edited.text, &path("components.Loot.gold"))
            .unwrap()
            .is_some()
    );

    let edited = remove_value(OGRE, &path("reactions[0]")).unwrap();
    assert_eq!(
        get_value(&edited.text, &path("reactions")).unwrap(),
        Some(json!([]))
    );

    assert_eq!(
        remove_value(OGRE, &path("components.Nope")).unwrap_err(),
        EditError::NotFound("components.Nope".into())
    );
}

#[test]
fn undo_and_redo_round_trip_the_text() {
    // Overwriting a value and undoing restores the exact original text.
    let edited = set_value(OGRE, &path("components.Health.max"), json!(1.0)).unwrap();
    let undone = apply_edit(&edited.text, &edited.edit.inverse()).unwrap();
    assert_eq!(undone.text, OGRE);
    let redone = apply_edit(&undone.text, &undone.edit.inverse()).unwrap();
    assert_eq!(redone.text, edited.text);

    // Removing a field and undoing brings the value back (position within the object may differ).
    let removed = remove_value(OGRE, &path("components.Loot.gold")).unwrap();
    let restored = apply_edit(&removed.text, &removed.edit.inverse()).unwrap();
    assert_eq!(
        get_value(&restored.text, &[]).unwrap(),
        get_value(OGRE, &[]).unwrap()
    );
    assert!(restored.text.contains("// hit points"));

    // Adding a field and undoing removes it again.
    let added = set_value(OGRE, &path("components.Faction"), json!("wild")).unwrap();
    let back = apply_edit(&added.text, &added.edit.inverse()).unwrap();
    assert_eq!(
        get_value(&back.text, &[]).unwrap(),
        get_value(OGRE, &[]).unwrap()
    );
}

#[test]
fn a_sequence_of_edits_undoes_in_reverse() {
    let mut text = OGRE.to_owned();
    let mut history = Vec::new();
    for (p, v) in [
        ("components.Health.max", json!(1.0)),
        ("components.Health.current", json!(2.0)),
        ("components.Faction", json!("wild")),
        ("transform.scale[0]", json!(9.0)),
    ] {
        let edited = set_value(&text, &path(p), v).unwrap();
        text = edited.text;
        history.push(edited.edit);
    }
    while let Some(edit) = history.pop() {
        text = apply_edit(&text, &edit.inverse()).unwrap().text;
    }
    assert_eq!(
        get_value(&text, &[]).unwrap(),
        get_value(OGRE, &[]).unwrap()
    );
    assert!(text.contains("// hit points") && text.contains("/* placement"));
}

#[test]
fn errors() {
    assert!(matches!(
        set_value("{ nope", &path("a"), json!(1)),
        Err(EditError::Syntax(_))
    ));
    assert_eq!(
        set_value("[1]", &path("a"), json!(1)).unwrap_err(),
        EditError::RootNotObject
    );
    assert_eq!(
        set_value(OGRE, &path("descendsFrom.x"), json!(1)).unwrap_err(),
        EditError::NotContainer("descendsFrom".into())
    );
}

#[test]
fn canonical_order_check_and_formatter() {
    assert_eq!(check_canonical_order(OGRE).unwrap(), None);

    let messy = r#"{
  // reactions first, wrongly
  "reactions": [],
  "components": { "Health": {} }, // the health
  "descendsFrom": "Actor",

  // headline for the rest
  "extra": 1,
  "transform": {}
}"#;
    let issue = check_canonical_order(messy).unwrap().unwrap();
    assert_eq!(
        issue.found,
        [
            "reactions",
            "components",
            "descendsFrom",
            "extra",
            "transform"
        ]
    );
    assert_eq!(
        issue.expected,
        [
            "descendsFrom",
            "transform",
            "components",
            "reactions",
            "extra"
        ]
    );
    assert!(issue.to_string().contains("canonical order is"));

    let fixed = canonicalize(messy).unwrap();
    assert_eq!(check_canonical_order(&fixed).unwrap(), None);
    // Comments moved with their fields.
    assert!(
        fixed.contains("// reactions first, wrongly\n  \"reactions\": []"),
        "{fixed}"
    );
    assert!(
        fixed.contains("\"components\": { \"Health\": {} }, // the health"),
        "{fixed}"
    );
    // Canonicalizing twice changes nothing more; already-canonical text is untouched.
    assert_eq!(canonicalize(&fixed).unwrap(), fixed);
    assert_eq!(canonicalize(OGRE).unwrap(), OGRE);
}
