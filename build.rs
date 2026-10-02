use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=MPV_PREFIX");
    println!("cargo:rerun-if-env-changed=SVP_DIR");
    println!("cargo:rerun-if-changed=build.rs");

    let third_party =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("third_party");

    // Link our own vapoursynth-enabled libmpv (scripts/build-libmpv.sh), not
    // the distro one — distro builds lack vapoursynth, which SVP needs.
    let prefix = std::env::var_os("MPV_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| third_party.join("mpv-prefix"));
    let lib_dir = prefix.join("lib");
    // Re-run once scripts/build-libmpv.sh produces (or rebuilds) the library.
    println!(
        "cargo:rerun-if-changed={}",
        lib_dir.join("libmpv.so").display()
    );

    if !lib_dir.join("libmpv.so").exists() {
        println!(
            "cargo:warning=no libmpv at {} — linking the system libmpv, which has no vapoursynth \
             (run scripts/build-libmpv.sh)",
            lib_dir.display()
        );
        return;
    }
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    // SVP's bundled VapourSynth R73 must win over the system R80 (its plugins
    // need API 3), plus the Python 3.12 that R73 embeds. This needs DT_RPATH,
    // not the default DT_RUNPATH: only RPATH also applies to libmpv's own
    // dependencies (libvsscript.so), not just the executable's.
    let svp_dir = std::env::var_os("SVP_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("SVP4")));
    let mut rpath = vec![third_party.join("svp-vapoursynth"), lib_dir];
    if let Some(svp_dir) = svp_dir {
        rpath.push(svp_dir.join("python"));
    }
    let rpath = rpath
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(":");
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,{rpath}");
}
