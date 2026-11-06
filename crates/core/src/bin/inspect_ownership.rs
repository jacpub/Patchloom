//! Read-only receipt inspection; original Ajour GPLv3 applies.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(root), Some(id), None) = (args.next(), args.next(), args.next()) else {
        eprintln!("Usage: inspect_ownership <copied-AddOns-root> <receipt-id>");
        std::process::exit(2);
    };
    let id = id.to_str().ok_or("Receipt identifier must be UTF-8")?;
    let preview = ajour_core::fs::inspect_ownership(std::path::Path::new(&root), id)?;
    serde_yaml::to_writer(std::io::stdout(), &preview)?;
    Ok(())
}
