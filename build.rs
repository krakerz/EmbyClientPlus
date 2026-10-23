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
    println!("cargo:rerun-if-env-changed=EMBYCLIENTPLUS_PORTABLE");
    println!("cargo:rerun-if-changed=build.rs");

    // Windows and macOS link the package manager's libmpv (MSYS2,
    // Homebrew), which has vapoursynth; nothing below applies to them.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rerun-if-changed=packaging/windows");
        embed_resource::compile("packaging/windows/embyclientplus.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("couldn't embed the Windows icon");
    }
    if target_os != "linux" {
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

    // libmpv links libvsscript.so. On Linux that is our stand-in
    // (packaging/vsshim), which loads SVP's VapourSynth R73 or the system's
    // at run time, as the app tells it (src/svp.rs). Built here so linking
    // never depends on a system VapourSynth either.
    let shim_dir = third_party.join("vsshim");
    build_vsshim(&shim_dir);

    // Release packages carry libmpv next to the binary and the stand-in in
    // lib/vsshim (packaging/embyclientplus.sh puts it on the library path),
    // so no absolute paths from the build machine may end up in the binary.
    if std::env::var_os("EMBYCLIENTPLUS_PORTABLE").is_some() {
        println!(
            "cargo:rustc-link-arg=-Wl,-rpath-link,{}",
            shim_dir.display()
        );
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,$ORIGIN/../lib");
        return;
    }

    // The stand-in has a folder of its own, so nothing there can shadow
    // another library. DT_RPATH, not the default DT_RUNPATH: only RPATH
    // also applies to libmpv's own dependencies (libvsscript.so), not just
    // the executable's.
    let rpath = [shim_dir, lib_dir]
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(":");
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,{rpath}");
}

/// Compiles packaging/vsshim/vsshim.c as `<dir>/libvsscript.so`.
fn build_vsshim(dir: &std::path::Path) {
    let source = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("packaging/vsshim/vsshim.c");
    println!("cargo:rerun-if-changed={}", source.display());
    std::fs::create_dir_all(dir).expect("couldn't create third_party/vsshim");
    let cc = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
    let status = std::process::Command::new(cc)
        .args([
            "-shared",
            "-fPIC",
            "-O2",
            "-Wall",
            "-Wl,-soname,libvsscript.so",
            "-o",
        ])
        .arg(dir.join("libvsscript.so"))
        .arg(&source)
        .arg("-ldl")
        .status()
        .expect("couldn't run cc for packaging/vsshim");
    assert!(
        status.success(),
        "compiling packaging/vsshim/vsshim.c failed"
    );
}
