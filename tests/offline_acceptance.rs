//! Harness-free offline acceptance entry, 2026-11-06; original GPLv3 applies.
//! Cargo supplies cfg(test) even with harness=false. Its own main, unlike
//! libtest's worker, owns the native event loop. Never invoked visibly in CI.

#[cfg(debug_assertions)]
#[path = "support/offline_preview.rs"]
pub mod inventory_preview;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(debug_assertions)]
    let code = inventory_preview::offline_acceptance_entry(&args);
    #[cfg(not(debug_assertions))]
    let code = {
        println!("Offline acceptance is disabled in release builds.");
        if args.iter().any(|arg| arg == "--visible") {
            2
        } else {
            0
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
}
