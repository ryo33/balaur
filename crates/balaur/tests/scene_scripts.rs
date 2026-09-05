//! `scene_scripts` walks a project's scene files for every script a node
//! attaches. It backs both `balaur check` and the LSP's diagnostics, so a
//! scene it cannot see is a checker that silently finds nothing.

/// Both forms a scene may attach a script in: a bare path, and a table whose
/// `source` names one (used when the node also tunes the script's exports).
#[test]
fn every_form_of_script_attachment_is_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scenes")).unwrap();
    std::fs::write(
        dir.path().join("project.eure"),
        "name = \"p\"\nmain_scene = \"scenes/main.eure\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("scenes/main.eure"),
        r#"
@ nodes[] {
  id: n_bare
  name: Bare
  script: scripts/bare.rn
}

@ nodes[] {
  id: n_tuned
  name: Tuned

  @ script
  source: scripts/tuned.rn
  props.speed = 3.5
}
"#,
    )
    .unwrap();

    let found = balaur::scene_scripts(dir.path());
    assert_eq!(
        found,
        vec![
            "scripts/bare.rn".to_string(),
            "scripts/tuned.rn".to_string()
        ],
        "expected both attached scripts, got {found:?}"
    );
}
