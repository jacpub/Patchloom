//! Explicit Patchloom recovery selection; original Ajour GPLv3 applies.
use std::{
    io::{self, Write},
    path::PathBuf,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = match (args.next(), args.next()) {
        (Some(path), None) => PathBuf::from(path),
        _ => {
            eprintln!("Usage: restore_originals <AddOns-directory>");
            std::process::exit(2);
        }
    };
    let restored = ajour_core::fs::restore_originals(&directory, |originals, retained| {
        println!(
            "Restore originals: {originals:?}\nMove current folders aside if present: {retained:?}"
        );
        print!("Type RESTORE to proceed, or press Enter to cancel: ");
        if io::stdout().flush().is_err() {
            return false;
        }
        let mut answer = String::new();
        io::stdin().read_line(&mut answer).is_ok() && answer.trim() == "RESTORE"
    })?;
    println!(
        "{}",
        if restored {
            "Originals restored. Replacement versions and recovery journals are retained; updates remain blocked pending cleanup."
        } else {
            "Cancelled. No addon folders changed."
        }
    );
    Ok(())
}
