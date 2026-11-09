//! Shared real app implementation with offline fixture wiring only.
//! No CLI startup, libtest worker or normal runtime entry is included here.
mod install {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/inventory_preview/install_runtime.rs"
    ));
    mod acceptance {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/inventory_preview/visible_acceptance.rs"
        ));
    }
    pub(crate) use acceptance::offline_acceptance_entry;
}
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/inventory_preview/runtime.rs"
));
pub(crate) use install::offline_acceptance_entry;
