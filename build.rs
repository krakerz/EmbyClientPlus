use std::path::PathBuf;

/// Embeds data/icons/*.svg (see scripts/update-icons.sh) as a table the
/// app installs into its icon theme at startup.
fn embed_icons() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("data/icons");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("data/icons is missing")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
        .collect();
    entries.sort();
    let mut table = String::from("pub static ICONS: &[(&str, &[u8])] = &[\n");
    for path in &entries {
        let name = path.file_stem().unwrap().to_string_lossy();
        table.push_str(&format!(
            "    ({name:?}, include_bytes!({:?})),\n",
            path.display().to_string()
        ));
    }
    table.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icons.rs");
    std::fs::write(out, table).expect("failed to write the icon table");
}

fn main() {
    embed_icons();
    println!("cargo:rerun-if-env-changed=MPV_PREFIX");
    println!("cargo:rerun-if-env-changed=SVP_DIR");
    println!("cargo:rerun-if-env-changed=EMBYCLIENTPLUS_PORTABLE");
    println!("cargo:rerun-if-changed=build.rs");

    // Windows and macOS link the package manager's libmpv (MSYS2,
    // Homebrew), which has vapoursynth; SVP's own copy is reached at run
    // time through packaging/vsshim instead of the paths set up below.
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os != "linux") {
        return;
    }

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

    // Release packages carry libmpv next to the binary and find SVP's
    // VapourSynth at launch (packaging/embyclientplus.sh), so no absolute
    // paths from the build machine may end up in the binary.
    if std::env::var_os("EMBYCLIENTPLUS_PORTABLE").is_some() {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,$ORIGIN/../lib");
        return;
    }

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
