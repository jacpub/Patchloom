//! Patchloom recovery inspection, 2026-10-27; original Ajour GPLv3 applies.
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = match (args.next(), args.next()) {
        (Some(path), None) => PathBuf::from(path),
        _ => {
            eprintln!("Usage: inspect_recovery <AddOns-directory>");
            std::process::exit(2);
        }
    };
    let report = ajour_core::fs::inspect_recovery(&directory)?;
    eprintln!(
        "Inspection only. Markers describe retained state, not a recommended recovery action."
    );
    serde_yaml::to_writer(std::io::stdout(), &report)?;
    Ok(())
}
