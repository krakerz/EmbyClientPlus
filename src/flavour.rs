//! Which Linux AppImage this is: "compat" (bundles glibc, for older
//! distros; its launcher sets `EMBYCLIENTPLUS_FLAVOUR=compat`) or the main
//! build. Every other build is "main".

use std::sync::OnceLock;

pub fn is_compat() -> bool {
    static COMPAT: OnceLock<bool> = OnceLock::new();
    *COMPAT.get_or_init(|| {
        std::env::var("EMBYCLIENTPLUS_FLAVOUR").is_ok_and(|flavour| flavour == "compat")
    })
}

pub fn name() -> &'static str {
    if is_compat() { "compat" } else { "main" }
}
