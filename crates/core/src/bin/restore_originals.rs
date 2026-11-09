//! Explicit Patchloom recovery selection; original Ajour GPLv3 applies.
//! Modified 2026-11-03; Router proposal integrated locally.
use ajour_core::fs::RecoveryRestoreOutcome;
use std::{
    io::{self, Write},
    path::PathBuf,
};

fn main() {
    if let Err(error) = run() {
        eprintln!(
            "Recovery failed: {error}. Inspect retained state before trying again; a failed restore may have moved some folders."
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = match (args.next(), args.next()) {
        (Some(path), None) => PathBuf::from(path),
        _ => {
            eprintln!("Usage: restore_originals <AddOns-directory>");
            std::process::exit(2);
        }
    };
    let approval = ajour_core::fs::preflight_restore_originals(&directory)?;
    println!("AddOns root: {:?}", approval.root());
    println!(
        "Restore originals: {:?}\nCurrent folder names checked by this plan: {:?}",
        approval.originals(),
        approval.move_aside_if_present()
    );
    println!("Folders moved aside are kept. A resumed restore continues from its recorded stage. Recovery journals remain; updates stay blocked pending cleanup. Keep Patchloom open until restoration finishes.");
    print!("Type RESTORE to proceed, or press Enter to cancel: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let consent = answer.trim() == "RESTORE";
    if !consent {
        println!("Cancelled. No addon folders changed.");
        return Ok(());
    }
    match ajour_core::fs::restore_originals_checked(
        &directory,
        &approval,
        |originals, retained| {
            originals == approval.originals()
                && retained == approval.move_aside_if_present()
                && consent
        },
    )? {
        RecoveryRestoreOutcome::Cancelled => println!("Cancelled. No addon folders changed."),
        RecoveryRestoreOutcome::Changed => {
            eprintln!("Recovery changed since preview. No restore was started; run again for a fresh preview.");
            std::process::exit(1);
        }
        RecoveryRestoreOutcome::RestoredRetained => println!(
            "Originals restored. Replacement versions and recovery journals are retained; updates remain blocked pending cleanup."
        ),
    }
    Ok(())
}
