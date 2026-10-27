//! Config-free public metadata preview; original Ajour GPLv3 applies.
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("Usage: check_github <owner/repository> <copied-WoW-root> <product-id>");
        std::process::exit(2);
    }
    let inventory = ajour_core::discovery::discover(std::path::Path::new(&args[1]))?;
    if !inventory.issues.is_empty() {
        return Err(std::io::Error::other("Resolve inventory issues first").into());
    }
    let install = inventory
        .installations
        .iter()
        .find(|i| i.product_id == args[2])
        .ok_or_else(|| std::io::Error::other("Selected product not discovered"))?;
    let candidate =
        async_std::task::block_on(ajour_core::github_releases::check(&args[0], install))?;
    serde_yaml::to_writer(std::io::stdout(), &candidate)?;
    Ok(())
}
