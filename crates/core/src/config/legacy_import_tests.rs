// Pure copy preflight regressions, 2026-11-09; original GPLv3 applies.
use super::*;

#[test]
fn legacy_root_and_selected_flavor_are_hints_without_defaulting_or_importing() {
    let source = b"wow:\n  directory: 'C:/Games/World of Warcraft'\n  flavor: Retail\n";
    let before = source.to_vec();
    let Preflight::Present(report) = inspect_copy(Some(source)).unwrap() else {
        panic!("missing report")
    };
    assert_eq!(report.directories.len(), 1);
    assert_eq!(
        report.directories[0].path_hint,
        "C:/Games/World of Warcraft"
    );
    assert!(report.directories[0].legacy_flavor.is_none());
    assert_eq!(
        report.selected_legacy_flavor.unwrap().kind,
        FlavorKind::RetailLabel
    );
    assert!(report
        .notices
        .contains(&Notice::LegacyRootNeedsProductSelection));
    assert_eq!(source, before.as_slice());
}

fn report(source: &str) -> Report {
    let Preflight::Present(report) = inspect_copy(Some(source.as_bytes())).unwrap() else {
        panic!("missing report")
    };
    report
}

#[test]
fn absence_empty_and_missing_fields_do_not_create_retail_defaults() {
    assert_eq!(inspect_copy(None).unwrap(), Preflight::Absent);
    for bytes in [b"".as_slice(), b" \r\n\t".as_slice()] {
        assert_eq!(inspect_copy(Some(bytes)), Err(PreflightError::Empty));
    }
    let report = report("wow: {}\n");
    assert!(report.selected_legacy_flavor.is_none());
    assert!(report.directories.is_empty());
    assert!(report.notices.contains(&Notice::NoDirectoryHints));
    assert_eq!(
        inspect_copy(Some(b"theme: blue\n")),
        Err(PreflightError::UnsupportedSchema)
    );
}

#[test]
fn directory_map_preserves_unicode_and_explicit_unknown_classic_and_test_labels() {
    let report = report("wow:\n  directories:\n    FutureFlavor: '/copie/未来'\n    RetailPTR: 'D:/WoW/_ptr_'\n    ClassicTbc: 'D:/WoW/_classic_'\n    Classic: '/copie/_classic_era_'\n    Retail: '/copie/été/世界/_retail_'\n");
    assert_eq!(report.directories.len(), 5);
    let kinds: Vec<_> = report
        .directories
        .iter()
        .map(|h| h.legacy_flavor.as_ref().unwrap().kind)
        .collect();
    assert_eq!(
        kinds,
        [
            FlavorKind::ClassicRequiresResolution,
            FlavorKind::ClassicRequiresResolution,
            FlavorKind::Unknown,
            FlavorKind::RetailLabel,
            FlavorKind::UnsupportedTestVariant
        ]
    );
    assert_eq!(report.directories[3].path_hint, "/copie/été/世界/_retail_");
    assert!(report.notices.contains(&Notice::ClassicNeedsResolution));
    assert!(report.notices.contains(&Notice::UnsupportedOrUnknownFlavor));
    assert!(report
        .notices
        .contains(&Notice::MultipleLocationsNeedResolution));
}

#[test]
fn legacy_single_and_map_locations_are_not_silently_combined_or_selected() {
    let report = report("wow:\n  directory: '/copies/game'\n  flavor: unknown-selected\n  directories:\n    Retail: '/copies/game'\n    ClassicEra: '/copies/game/_classic_/'\n");
    assert_eq!(report.directories.len(), 3);
    assert_eq!(
        report.selected_legacy_flavor.unwrap().kind,
        FlavorKind::Unknown
    );
    assert!(report.directories[0].legacy_flavor.is_none());
    assert!(report.notices.contains(&Notice::RepeatedPathHints));
    assert!(report.directories[1].classic_folder_ambiguous);
}

#[test]
fn unrelated_values_are_not_echoed_and_other_information_is_explicitly_uninspected() {
    let report = report("api_key: do-not-echo\naddons: {ignored: {Retail: [123]}}\nwow:\n  extra: private-account-value\n  directories: {}\n");
    assert_eq!(report.uninspected_root_fields, 2);
    assert_eq!(report.uninspected_wow_fields, 1);
    assert!(report.notices.contains(&Notice::OtherSettingsNotInspected));
    let output = serde_json::to_string(&report).unwrap();
    assert!(!output.contains("do-not-echo"));
    assert!(!output.contains("private-account-value"));
}

#[test]
fn source_digest_identifies_exact_bytes_without_authorizing_any_import() {
    let first = report("wow: {}");
    let second = report("wow: {}\n");
    assert_eq!(first.source_sha256.len(), 64);
    assert_ne!(first.source_sha256, second.source_sha256);
    assert_eq!(first.directories, second.directories);
    assert_eq!(first.source_bytes, 7);
}

#[test]
fn malformed_types_versions_and_non_string_keys_fail_explicitly() {
    for source in [
        "[",
        "wow: [",
        "wow: [1,2]",
        "wow: 1",
        "wow: {directory: 123}",
        "wow: {directories: [x]}",
        "wow: {directories: {Retail: false}}",
        "wow: {flavor: true}",
        "version: 1\nwow: {}",
        "schema_version: 2\nwow: {}",
        "schema: future\nwow: {}",
        "true: ignored\nwow: {}",
        "wow: {future: {1: unsupported}}",
    ] {
        assert!(
            inspect_copy(Some(source.as_bytes())).is_err(),
            "unexpected acceptance of {}",
            source
        );
    }
    assert_eq!(
        inspect_copy(Some(&[0xff, 0xfe])),
        Err(PreflightError::InvalidYaml)
    );
}

#[test]
fn duplicate_keys_are_rejected_before_value_overwrite_even_when_escaped() {
    for source in [
        "wow: {}\nwow: {}",
        "wow: {directory: a, directory: b}",
        "wow: {directories: {Retail: a, Retail: b}}",
        "wow: {}\n\"w\\u006fw\": {}",
    ] {
        assert_eq!(
            inspect_copy(Some(source.as_bytes())),
            Err(PreflightError::DuplicateKey)
        );
    }
    // Reusing a key in a different mapping is not a duplicate.
    assert!(inspect_copy(Some(b"wow: {directories: {Retail: a}}\nother: {Retail: b}")).is_ok());
}

#[test]
fn aliases_cycles_tags_complex_keys_and_multiple_documents_are_not_expanded() {
    for source in [
        "wow: &a {directory: x}\nother: *a",
        "wow: &a [*a]",
        "wow: {directory: !!str /x}",
        "wow: !custom {directory: /x}",
        "wow: {<<: {directory: /x}}",
        "%YAML 1.1\n---\nwow: {}",
        "? [complex, key]\n: value\nwow: {}",
        "wow: {}\n---\nwow: {}",
    ] {
        assert!(matches!(
            inspect_copy(Some(source.as_bytes())),
            Err(PreflightError::UnsupportedYaml)
        ));
    }
}

#[test]
fn input_depth_scalar_event_hint_and_label_limits_fail_closed() {
    assert_eq!(
        inspect_copy(Some(&vec![b' '; MAX_INPUT_BYTES + 1])),
        Err(PreflightError::Limit("input bytes"))
    );
    let deep = format!("wow: {{extra: {}0{}}}", "[".repeat(40), "]".repeat(40));
    assert_eq!(
        inspect_copy(Some(deep.as_bytes())),
        Err(PreflightError::Limit("YAML depth"))
    );
    let huge = format!("wow: {{directory: '{}'}}", "x".repeat(MAX_FIELD_BYTES + 1));
    assert!(matches!(
        inspect_copy(Some(huge.as_bytes())),
        Err(PreflightError::Limit(_))
    ));
    let wide = format!("wow: {{extra: [{}]}}", "0,".repeat(5000));
    assert!(matches!(
        inspect_copy(Some(wide.as_bytes())),
        Err(PreflightError::Limit(_))
    ));
    let mut many = "wow:\n  directories:\n".to_owned();
    for n in 0..=MAX_HINTS {
        many.push_str(&format!("    F{n}: '/copy/{n}'\n"));
    }
    assert_eq!(
        inspect_copy(Some(many.as_bytes())),
        Err(PreflightError::Limit("directory hints"))
    );
    let label = format!("wow: {{flavor: '{}'}}", "x".repeat(65));
    assert_eq!(
        inspect_copy(Some(label.as_bytes())),
        Err(PreflightError::Limit("flavor label"))
    );
}

#[test]
fn unsafe_paths_and_wrong_string_types_are_not_truncated_or_normalized() {
    for path in [
        "",
        " relative ",
        "../copy",
        "/copy/_classic_/.",
        "C:relative",
        "file:///copy",
        "https://invalid/x",
        "\\\\server\\share",
        "\\rooted-without-drive",
        "\\\\?\\C:\\copy",
        "//server/share",
        "x\u{202e}bad",
        "x\0bad",
    ] {
        let source = serde_yaml::to_string(&serde_json::json!({"wow":{"directory":path}})).unwrap();
        assert_eq!(
            inspect_copy(Some(source.as_bytes())),
            Err(PreflightError::UnsafePath)
        );
    }
    let report = report("wow: {directory: 'relative-copy/_retail_'}");
    assert_eq!(report.directories[0].path_hint, "relative-copy/_retail_");
}

#[test]
fn accepted_maximum_report_is_bounded_and_keeps_complete_path_hints() {
    let mut source = "wow:\n  directories:\n".to_owned();
    for n in 0..MAX_HINTS {
        source.push_str(&format!("    F{n}: '/copies/{n}/{}'\n", "界".repeat(1300)));
    }
    assert!(source.len() <= MAX_INPUT_BYTES);
    let report = report(&source);
    assert_eq!(report.directories.len(), MAX_HINTS);
    assert!(report
        .directories
        .iter()
        .all(|h| h.path_hint.ends_with(&"界".repeat(1300))));
    assert!(report.notices.len() <= 7);
    assert!(serde_json::to_vec(&report).unwrap().len() < MAX_INPUT_BYTES * 3);
}

#[test]
fn copied_path_hints_and_source_files_are_never_used_as_operations() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("missing-game");
    let copy = fixture.path().join("copied-settings.yml");
    let source =
        serde_yaml::to_vec(&serde_json::json!({"wow":{"directory":root.to_string_lossy()}}))
            .unwrap();
    std::fs::write(&copy, &source).unwrap();
    let report = inspect_copy(Some(&source)).unwrap();
    assert!(matches!(report, Preflight::Present(_)));
    assert!(!root.exists());
    assert_eq!(std::fs::read(&copy).unwrap(), source);
    assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 1);
    // No Config/global-path, filesystem or network API is called by the pure
    // implementation; these snapshots separately verify fixture preservation.
}

#[test]
fn backend_guards_stop_consuming_after_rejection() {
    use std::cell::Cell;
    let count = Cell::new(0usize);
    let source = format!("wow: &anchor {{}}\n#{}", "x".repeat(30_000));
    assert_eq!(
        guard_tokens(source.chars().inspect(|_| count.set(count.get() + 1))),
        Err(PreflightError::UnsupportedYaml)
    );
    assert!(
        count.get() < 1000,
        "anchor rejection consumed {} characters",
        count.get()
    );
    // Stay within the backend's flow-depth limit. Its scanner may look ahead
    // through this bounded collection; the unread suffix proves rejection
    // does not eagerly consume the rest of the supplied input.
    let source = format!(
        "wow: {{extra: {}0{}}}\n#{}",
        "[".repeat(64),
        "]".repeat(64),
        "x".repeat(30_000)
    );
    count.set(0);
    assert_eq!(
        guard_events(source.chars().inspect(|_| count.set(count.get() + 1))),
        Err(PreflightError::Limit("YAML depth"))
    );
    assert!(
        count.get() < source.len() / 2,
        "depth rejection consumed {} characters",
        count.get()
    );
    let source = format!(
        "wow: {{extra: [{}]}}\n#{}",
        "0,".repeat(5000),
        "x".repeat(30_000)
    );
    count.set(0);
    assert_eq!(
        guard_events(source.chars().inspect(|_| count.set(count.get() + 1))),
        Err(PreflightError::Limit("YAML events"))
    );
    assert!(
        count.get() < source.len() / 2,
        "event rejection consumed {} characters",
        count.get()
    );
}

#[test]
fn backend_flow_limit_is_a_rejection_not_an_event_depth_guard_pass() {
    let source = format!("wow: {{extra: {}0{}}}", "[".repeat(2000), "]".repeat(2000));
    let mut parser = Parser::new(source.chars());
    for _ in 0..MAX_EVENTS {
        match parser.next() {
            Err(error) => {
                assert!(error.to_string().contains("recursion limit exceeded"));
                assert_eq!(
                    inspect_copy(Some(source.as_bytes())),
                    Err(PreflightError::InvalidYaml)
                );
                return;
            }
            Ok((Event::StreamEnd, _)) => panic!("compact fixture unexpectedly accepted"),
            _ => {}
        }
    }
    panic!("compact fixture did not produce the observed backend error");
}
