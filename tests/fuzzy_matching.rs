//! Fuzzy matcher compatibility across dependency patches, 2026-11-05.
//! Original Ajour GPLv3 license and attribution apply.
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use std::sync::Arc;

#[test]
fn shared_fuzzy_matcher_preserves_scores_and_indices_across_threads() {
    let fixtures = [
        ("Deadly Boss Mods", "dbm", Some(vec![0, 7, 12])),
        ("Atlas", "Atlas", Some(vec![0, 1, 2, 3, 4])),
        ("Über Addon", "ÜA", Some(vec![0, 5])),
        ("Atlas", "zzz", None),
        ("", "a", None),
        ("Atlas", "", Some(vec![])),
    ];
    let matcher = Arc::new(SkimMatcherV2::default());
    let baseline: Vec<_> = fixtures
        .iter()
        .map(|(text, query, indices)| {
            let result = matcher.fuzzy_indices(text, query);
            assert_eq!(result.as_ref().map(|(_, found)| found), indices.as_ref());
            assert_eq!(
                matcher.fuzzy_match(text, query),
                result.as_ref().map(|(score, _)| *score)
            );
            result
        })
        .collect();
    // Captured before/after the exact patch update as an external test receipt.
    println!("FUZZY_COMPAT {baseline:?}");
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let matcher = Arc::clone(&matcher);
            let baseline = baseline.clone();
            let fixtures = fixtures.clone();
            std::thread::spawn(move || {
                for _ in 0..100 {
                    for ((text, query, _), expected) in fixtures.iter().zip(&baseline) {
                        assert_eq!(&matcher.fuzzy_indices(text, query), expected);
                        assert_eq!(
                            matcher.fuzzy_match(text, query),
                            expected.as_ref().map(|(score, _)| *score)
                        );
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
}
