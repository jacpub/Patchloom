//! Parallel parsing compatibility across Crossbeam patches, 2026-11-05.
//! Original Ajour GPLv3 attribution applies.
use ajour_core::parse::parse_toc_path;
use rayon::prelude::*;

#[test]
fn parallel_toc_parsing_preserves_metadata_order_and_source_files() {
    let root = tempfile::tempdir().unwrap();
    let fixtures: Vec<_> = (0..64)
        .map(|i| {
            let folder = root.path().join(format!("Fixture{i}"));
            std::fs::create_dir(&folder).unwrap();
            let path = folder.join("Fixture.toc");
            let text = format!(
                "## Title: |cff00ff00Fixture {i}|r\n## Version: 1.{i}\n## Dependencies: Shared\n"
            );
            std::fs::write(&path, &text).unwrap();
            (path, text)
        })
        .collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for _ in 0..8 {
        let parsed: Vec<_> = pool.install(|| {
            fixtures
                .par_iter()
                .map(|(path, _)| parse_toc_path(path).unwrap())
                .collect()
        });
        assert_eq!(parsed.len(), fixtures.len());
        for (i, addon) in parsed.iter().enumerate() {
            assert_eq!(addon.id, format!("Fixture{i}"));
            assert_eq!(addon.title, format!("Fixture {i}"));
            assert_eq!(addon.version, Some(format!("1.{i}")));
            assert_eq!(addon.dependencies, ["Shared"]);
        }
    }
    for (path, original) in fixtures {
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }
}
