use serde_json::json;
use struction_data::{DataError, ErrorKind, edit::parse_path};
use struction_editor::{EditRequest, EditSession, Field, SessionError};

fn fixture() -> (tempfile::TempDir, EditSession) {
    let dir = tempfile::tempdir().unwrap();
    for file in ["a.jsonc", "b.jsonc"] {
        std::fs::write(
            dir.path().join(file),
            "// Comment.\n{\"array\":[1,2,3],\"x\":0}\n",
        )
        .unwrap();
    }
    let session = EditSession::new(dir.path());
    (dir, session)
}

#[test]
fn stale_revisions_cannot_overwrite_an_external_edit() {
    let (dir, mut session) = fixture();
    let revision = struction_editor::session::revision(&session.read("a.jsonc").unwrap());
    std::fs::write(dir.path().join("a.jsonc"), "{\"x\":99}").unwrap();
    let request = EditRequest::Set {
        file: "a.jsonc".into(),
        path: vec![Field::Key("x".into())],
        value: json!(1),
        label: "Set".into(),
        group: None,
        revision: Some(revision),
    };
    assert!(matches!(
        session.apply_checked(request, |_| Ok(())),
        Err(SessionError::Conflict(_))
    ));
    assert_eq!(session.read("a.jsonc").unwrap(), "{\"x\":99}");
    assert!(!session.history().can_undo());
}

#[test]
fn noop_preserves_redo_and_array_removal_undo_is_exact() {
    let (_dir, mut session) = fixture();
    let original = session.read("a.jsonc").unwrap();
    session
        .remove("a.jsonc", &parse_path("array[1]"), "Remove middle")
        .unwrap();
    assert_eq!(
        struction_data::edit::get_value(&session.read("a.jsonc").unwrap(), &parse_path("array"))
            .unwrap(),
        Some(json!([1, 3]))
    );
    session.undo().unwrap();
    assert_eq!(session.read("a.jsonc").unwrap(), original);
    session
        .set("a.jsonc", &parse_path("x"), json!(0), "Noop", None)
        .unwrap();
    assert!(session.history().can_redo());
    session.redo().unwrap();
    assert_eq!(
        struction_data::edit::get_value(&session.read("a.jsonc").unwrap(), &parse_path("array"))
            .unwrap(),
        Some(json!([1, 3]))
    );
}

#[test]
fn grouped_multifile_undo_validates_before_writing_and_can_be_retried() {
    let (_dir, mut session) = fixture();
    let original = session.read("a.jsonc").unwrap();
    for file in ["a.jsonc", "b.jsonc"] {
        session
            .set(file, &parse_path("x"), json!(4), "Both", Some("group"))
            .unwrap();
    }
    session.end_group();
    let applied = session.read("a.jsonc").unwrap();
    let error = session.undo_checked(|sources| {
        assert_eq!(sources.len(), 2);
        Err(vec![DataError::new(
            ErrorKind::Io("reject candidate".into()),
            None,
        )])
    });
    assert!(matches!(error, Err(SessionError::Validation(_))));
    assert_eq!(session.read("a.jsonc").unwrap(), applied);
    assert_eq!(session.read("b.jsonc").unwrap(), applied);
    assert!(session.history().can_undo());
    assert!(!session.history().can_redo());
    session.undo().unwrap();
    assert_eq!(session.read("a.jsonc").unwrap(), original);
    assert_eq!(session.read("b.jsonc").unwrap(), original);
}

#[test]
fn paths_and_scaffolds_cannot_alias_or_overwrite_sources() {
    let (_dir, mut session) = fixture();
    for path in [
        "",
        "../a.jsonc",
        "./a.jsonc",
        "/a.jsonc",
        "dir//a.jsonc",
        "dir/../a.jsonc",
        "dir\\a.jsonc",
    ] {
        assert!(
            matches!(session.path_of(path), Err(SessionError::InvalidPath(_))),
            "{path}"
        );
    }
    let before = session.read("a.jsonc").unwrap();
    assert!(session.create_file("a.jsonc", "{}").is_err());
    assert_eq!(session.read("a.jsonc").unwrap(), before);
    session.create_file("nested/new.jsonc", "{}").unwrap();
    session.set_playing(true);
    assert!(matches!(
        session.create_file("blocked.jsonc", "{}"),
        Err(SessionError::Playing)
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_bypass_revision_or_history_tracking() {
    let (dir, session) = fixture();
    std::os::unix::fs::symlink(dir.path().join("a.jsonc"), dir.path().join("alias.jsonc")).unwrap();
    assert!(matches!(
        session.read("alias.jsonc"),
        Err(SessionError::InvalidPath(_))
    ));
}
