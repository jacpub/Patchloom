//! Read-only Patchloom inventory; original Ajour GPLv3 applies.
use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = match (args.next(), args.next()) {
        (Some(path), None) => PathBuf::from(path),
        _ => {
            eprintln!("Usage: discover_wow <copied-WoW-root>");
            std::process::exit(2);
        }
    };
    let report = ajour_core::discovery::discover(&root)?;
    serde_yaml::to_writer(std::io::stdout(), &report)?;
    if !report.issues.is_empty()
        || report
            .installations
            .iter()
            .any(|i| i.addons.iter().any(|a| a.issue.is_some()))
    {
        std::process::exit(1);
    }
    Ok(())
}
