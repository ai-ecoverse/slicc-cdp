#[test]
fn session_atomic_publication_fake_keeps_generation_metadata_during_an_open_writer() {
    let root = std::env::temp_dir().join(format!("slicc-state-publication-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let path = root.join("slicc/playwright-cli/sessions/current.json");
    let mut previous = Session::default();
    previous.webmcp.insert("TAB".into(), json!({"owner":"bridge-owner","sessions":[{"target":"TAB","bridge":"bridge-owner","documents":{"TAB:L1":{"origin":"https://ordinary.test","policy":"tools=(*)"}}}]}));
    write_at(&path, &previous).unwrap();
    let observed = path.clone();
    let expected = previous.webmcp.clone();
    PRIVATE_OPEN.with(|hook| *hook.borrow_mut() = Some(Box::new(move |opened| {
        assert_ne!(opened, observed);
        assert_eq!(fs::metadata(opened).unwrap().len(), 0);
        assert_eq!(load_at(&observed).webmcp, expected);
        serde_json::from_str::<Value>(&fs::read_to_string(&observed).unwrap()).unwrap();
    })));
    struct Reset;
    impl Drop for Reset { fn drop(&mut self) { PRIVATE_OPEN.with(|hook| hook.borrow_mut().take()); } }
    let _reset = Reset;
    let mut current = previous.clone();
    current.current = Some("TAB".into());
    write_at(&path, &current).unwrap();
    assert_eq!(load_at(&path).current.as_deref(), Some("TAB"));
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_atomic_publication_preserves_the_old_generation_for_an_open_reader() {
    use std::io::Read;
    let root = std::env::temp_dir().join(format!("slicc-state-reader-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let path = root.join("state.json");
    write_private_file(&path, "{\"generation\":1}").unwrap();
    let mut opened = fs::File::open(&path).unwrap();
    write_private_file(&path, "{\"generation\":2,\"complete\":true}").unwrap();
    let mut old = String::new();
    opened.read_to_string(&mut old).unwrap();
    assert_eq!(old, "{\"generation\":1}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\"generation\":2,\"complete\":true}");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_atomic_publication_failure_preserves_the_destination_and_removes_its_stage() {
    let root = std::env::temp_dir().join(format!("slicc-state-failure-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let path = root.join("directory.json");
    fs::create_dir_all(&path).unwrap();
    assert!(write_private_file(&path, "{\"generation\":2}").is_err());
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

