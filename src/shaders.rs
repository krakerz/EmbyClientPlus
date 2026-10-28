//! GLSL shader presets for mpv: the bundled shaders (FSR, Anime4K, chroma and luma upscalers, sharpeners), plus the
//! user's own from `~/.config/embyclientplus/shaders/<group>/`. Each group
//! has at most one preset active; active presets of different groups stack.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::config::ShaderSettings;

pub const FSR: &str = "fsr";
pub const ANIME4K: &str = "anime4k";
pub const CHROMA: &str = "chroma";
pub const LUMA: &str = "luma";
/// Copies of the luma shaders without their `//!WHEN` gates, for "Run upscalers at any size".
const LUMA_ALWAYS: &str = "luma-always";
pub const SHARPEN: &str = "sharpen";
/// Custom group ids: this prefix plus the folder name.
const CUSTOM_PREFIX: &str = "custom:";

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub presets: Vec<Preset>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub id: String,
    pub name: String,
    /// Applied in this order.
    pub files: Vec<PathBuf>,
}

/// FSR's sharpness is a `#define`, so each preset is its own copy of the
/// file. 0.0 is the sharpest; the shader's own default is 0.2.
const FSR_PRESETS: [(&str, &str, &str); 3] = [
    ("sharp", "Sharp", "0.0"),
    ("balanced", "Balanced", "0.2"),
    ("soft", "Soft", "1.0"),
];

/// Anime4K's official mpv modes (its GLSL_Mac_Linux templates): id, name,
/// shader chain for lower-end GPUs ("Fast") and for higher-end ones ("HQ").
const ANIME4K_MODES: [(&str, &str, &[&str], &[&str]); 6] = [
    (
        "a",
        "Mode A (1080p)",
        &[
            "Clamp_Highlights",
            "Restore_CNN_M",
            "Upscale_CNN_x2_M",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Upscale_CNN_x2_S",
        ],
        &[
            "Clamp_Highlights",
            "Restore_CNN_VL",
            "Upscale_CNN_x2_VL",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Upscale_CNN_x2_M",
        ],
    ),
    (
        "b",
        "Mode B (720p)",
        &[
            "Clamp_Highlights",
            "Restore_CNN_Soft_M",
            "Upscale_CNN_x2_M",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Upscale_CNN_x2_S",
        ],
        &[
            "Clamp_Highlights",
            "Restore_CNN_Soft_VL",
            "Upscale_CNN_x2_VL",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Upscale_CNN_x2_M",
        ],
    ),
    (
        "c",
        "Mode C (480p)",
        &[
            "Clamp_Highlights",
            "Upscale_Denoise_CNN_x2_M",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Upscale_CNN_x2_S",
        ],
        &[
            "Clamp_Highlights",
            "Upscale_Denoise_CNN_x2_VL",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Upscale_CNN_x2_M",
        ],
    ),
    (
        "aa",
        "Mode A+A",
        &[
            "Clamp_Highlights",
            "Restore_CNN_M",
            "Upscale_CNN_x2_M",
            "Restore_CNN_S",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Upscale_CNN_x2_S",
        ],
        &[
            "Clamp_Highlights",
            "Restore_CNN_VL",
            "Upscale_CNN_x2_VL",
            "Restore_CNN_M",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Upscale_CNN_x2_M",
        ],
    ),
    (
        "bb",
        "Mode B+B",
        &[
            "Clamp_Highlights",
            "Restore_CNN_Soft_M",
            "Upscale_CNN_x2_M",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Restore_CNN_Soft_S",
            "Upscale_CNN_x2_S",
        ],
        &[
            "Clamp_Highlights",
            "Restore_CNN_Soft_VL",
            "Upscale_CNN_x2_VL",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Restore_CNN_Soft_M",
            "Upscale_CNN_x2_M",
        ],
    ),
    (
        "ca",
        "Mode C+A",
        &[
            "Clamp_Highlights",
            "Upscale_Denoise_CNN_x2_M",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Restore_CNN_S",
            "Upscale_CNN_x2_S",
        ],
        &[
            "Clamp_Highlights",
            "Upscale_Denoise_CNN_x2_VL",
            "AutoDownscalePre_x2",
            "AutoDownscalePre_x4",
            "Restore_CNN_M",
            "Upscale_CNN_x2_M",
        ],
    ),
];

macro_rules! anime4k {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../data/shaders/anime4k/Anime4K_", $name, ".glsl")))),*]
    };
}

const ANIME4K_FILES: &[(&str, &str)] = anime4k!(
    "Clamp_Highlights",
    "Restore_CNN_M",
    "Restore_CNN_S",
    "Restore_CNN_VL",
    "Restore_CNN_Soft_M",
    "Restore_CNN_Soft_S",
    "Restore_CNN_Soft_VL",
    "Upscale_CNN_x2_S",
    "Upscale_CNN_x2_M",
    "Upscale_CNN_x2_VL",
    "Upscale_Denoise_CNN_x2_M",
    "Upscale_Denoise_CNN_x2_VL",
    "AutoDownscalePre_x2",
    "AutoDownscalePre_x4",
);

const FSR_SOURCE: &str = include_str!("../data/shaders/fsr/FSR.glsl");

macro_rules! bundled {
    ($dir:literal; $($file:literal),* $(,)?) => {
        &[$(($dir, $file, include_str!(concat!("../data/shaders/", $dir, "/", $file)))),*]
    };
}

/// Bundled files written as they are (folder, file name, text), licence
/// notices included so they sit next to the installed shaders.
const BUNDLED_FILES: &[&[(&str, &str, &str)]] = &[
    bundled!("anime4k"; "LICENSE"),
    bundled!("fsr"; "LICENSE"),
    bundled!("chroma"; "KrigBilateral.glsl", "CfL_Prediction_Lite.glsl", "FastBilateral.glsl", "LICENSE"),
    bundled!(
        "luma";
        "ravu-lite-ar-r4.hook",
        "ravu-lite-r4.hook",
        "nnedi3-nns16-win8x4.hook",
        "nnedi3-nns32-win8x4.hook",
        "nnedi3-nns64-win8x4.hook",
        "FSRCNNX_x2_8-0-4-1.glsl",
        "FSRCNNX_x2_16-0-4-1.glsl",
        "LICENSE",
    ),
    bundled!("sharpen"; "adaptive-sharpen.glsl", "CAS.glsl", "CAS-scaled.glsl", "LICENSE"),
];

type BundledPreset = (&'static str, &'static str, &'static [&'static str]);

/// Groups made of `BUNDLED_FILES`: id (= folder), name, presets (id, name,
/// files in order). Fragment versions only: macOS GL has no compute shaders.
/// ArtCNN is left out: its compute shaders abort mpv 0.41's vo=gpu.
const BUNDLED_GROUPS: [(&str, &str, &[BundledPreset]); 3] = [
    (
        CHROMA,
        "Chroma upscaling",
        &[
            ("krig", "KrigBilateral", &["KrigBilateral.glsl"]),
            (
                "cfl-lite",
                "Chroma from Luma (Lite)",
                &["CfL_Prediction_Lite.glsl"],
            ),
            ("fast-bilateral", "Fast Bilateral", &["FastBilateral.glsl"]),
        ],
    ),
    (
        LUMA,
        "Luma upscaler",
        &[
            (
                "ravu-lite-ar",
                "RAVU Lite (anti-ringing)",
                &["ravu-lite-ar-r4.hook"],
            ),
            ("ravu-lite", "RAVU Lite", &["ravu-lite-r4.hook"]),
            (
                "nnedi3-16",
                "NNEDI3 · 16 neurons",
                &["nnedi3-nns16-win8x4.hook"],
            ),
            (
                "nnedi3-32",
                "NNEDI3 · 32 neurons",
                &["nnedi3-nns32-win8x4.hook"],
            ),
            (
                "nnedi3-64",
                "NNEDI3 · 64 neurons",
                &["nnedi3-nns64-win8x4.hook"],
            ),
            ("fsrcnnx-8", "FSRCNNX 8", &["FSRCNNX_x2_8-0-4-1.glsl"]),
            ("fsrcnnx-16", "FSRCNNX 16", &["FSRCNNX_x2_16-0-4-1.glsl"]),
        ],
    ),
    (
        SHARPEN,
        "Sharpening",
        // CAS.glsl runs only at 1:1, CAS-scaled only when upscaling.
        &[
            ("cas", "AMD CAS", &["CAS.glsl", "CAS-scaled.glsl"]),
            ("adaptive", "Adaptive sharpen", &["adaptive-sharpen.glsl"]),
        ],
    ),
];

/// A cost/usage note for a built-in group, shown in Preferences.
pub fn hint(group: &str) -> Option<&'static str> {
    Some(match group {
        FSR => "light; upscales and sharpens",
        ANIME4K => "medium to heavy; for anime",
        CHROMA => "sharper colour edges; light (Fast Bilateral) to medium (KrigBilateral)",
        LUMA => {
            "doubles detail before scaling; RAVU Lite is light, NNEDI3 64 and FSRCNNX 16 are heavy (too heavy for the Steam Deck)"
        }
        SHARPEN => "CAS light, Adaptive medium; FSR already sharpens",
        _ => return None,
    })
}

/// Where the user's own shader groups live.
pub fn custom_dir() -> Option<PathBuf> {
    crate::config::config_dir()
        .ok()
        .map(|dir| dir.join("shaders"))
}

fn builtin_dir() -> Option<PathBuf> {
    crate::config::cache_dir().map(|dir| dir.join("shaders"))
}

/// Every group, in menu order: FSR, Anime4K, chroma, luma, sharpening, then custom folders by name.
/// Built-ins are written to the cache first (mpv loads shaders by path).
pub fn groups() -> Vec<Group> {
    let mut groups = Vec::new();
    match builtin_dir().map(|dir| install_builtins(&dir).map(|()| dir)) {
        Some(Ok(dir)) => groups.extend(builtin_groups(&dir)),
        Some(Err(e)) => tracing::warn!("could not install the bundled shaders: {e}"),
        None => {}
    }
    if let Some(dir) = custom_dir() {
        groups.extend(custom_groups(&dir));
    }
    groups
}

/// Writes the bundled shaders under `dir`, skipping files already current.
fn install_builtins(dir: &Path) -> std::io::Result<()> {
    let write = |path: PathBuf, text: &str| -> std::io::Result<()> {
        if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
            return Ok(());
        }
        std::fs::create_dir_all(path.parent().unwrap_or(dir))?;
        std::fs::write(path, text)
    };
    for (name, text) in ANIME4K_FILES {
        write(dir.join(ANIME4K).join(format!("Anime4K_{name}.glsl")), text)?;
    }
    for (id, _, sharpness) in FSR_PRESETS {
        write(
            dir.join(FSR).join(format!("FSR-{id}.glsl")),
            &fsr_with(sharpness),
        )?;
    }
    for (folder, name, text) in BUNDLED_FILES.iter().flat_map(|files| files.iter()) {
        write(dir.join(folder).join(name), text)?;
        if *folder == LUMA && *name != "LICENSE" {
            write(dir.join(LUMA_ALWAYS).join(name), &strip_when(text))?;
        }
    }
    Ok(())
}

/// The shader text without its `//!WHEN` lines, so every pass runs whatever the video and screen sizes.
fn strip_when(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|line| !line.trim_start().starts_with("//!WHEN"))
        .collect()
}

fn fsr_with(sharpness: &str) -> String {
    FSR_SOURCE.replacen(
        "#define SHARPNESS 0.2 ",
        &format!("#define SHARPNESS {sharpness} "),
        1,
    )
}

fn builtin_groups(dir: &Path) -> Vec<Group> {
    let fsr = Group {
        id: FSR.into(),
        name: "FSR".into(),
        presets: FSR_PRESETS
            .iter()
            .map(|(id, name, _)| Preset {
                id: (*id).into(),
                name: (*name).into(),
                files: vec![dir.join(FSR).join(format!("FSR-{id}.glsl"))],
            })
            .collect(),
    };
    let chain = |names: &[&str]| -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| dir.join(ANIME4K).join(format!("Anime4K_{name}.glsl")))
            .collect()
    };
    let mut anime4k = Vec::new();
    for (id, name, fast, hq) in ANIME4K_MODES {
        anime4k.push(Preset {
            id: format!("{id}-fast"),
            name: format!("{name} · Fast"),
            files: chain(fast),
        });
        anime4k.push(Preset {
            id: format!("{id}-hq"),
            name: format!("{name} · HQ"),
            files: chain(hq),
        });
    }
    let mut groups = vec![
        fsr,
        Group {
            id: ANIME4K.into(),
            name: "Anime4K".into(),
            presets: anime4k,
        },
    ];
    groups.extend(BUNDLED_GROUPS.iter().map(|(id, name, presets)| {
        Group {
            id: (*id).into(),
            name: (*name).into(),
            presets: presets
                .iter()
                .map(|(preset, preset_name, files)| Preset {
                    id: (*preset).into(),
                    name: (*preset_name).into(),
                    files: files.iter().map(|file| dir.join(id).join(file)).collect(),
                })
                .collect(),
        }
    }));
    groups
}

/// One group per folder in `dir`; each `.glsl` file in it is a preset, and
/// so is each subfolder (its `.glsl` files applied in name order).
fn custom_groups(dir: &Path) -> Vec<Group> {
    let mut groups: Vec<Group> = sorted_entries(dir)
        .into_iter()
        .filter(|path| path.is_dir())
        .filter_map(|folder| {
            let name = folder.file_name()?.to_string_lossy().into_owned();
            let presets: Vec<Preset> = sorted_entries(&folder)
                .into_iter()
                .filter_map(|path| {
                    let files = if path.is_dir() {
                        sorted_entries(&path)
                            .into_iter()
                            .filter(|f| is_glsl(f))
                            .collect()
                    } else if is_glsl(&path) {
                        vec![path.clone()]
                    } else {
                        return None;
                    };
                    let name = path.file_stem()?.to_string_lossy().into_owned();
                    (!files.is_empty()).then(|| Preset {
                        id: name.clone(),
                        name,
                        files,
                    })
                })
                .collect();
            (!presets.is_empty()).then(|| Group {
                id: format!("{CUSTOM_PREFIX}{name}"),
                name,
                presets,
            })
        })
        .collect();
    groups.sort_by_key(|group| group.name.to_lowercase());
    groups
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    entries.retain(|path| {
        !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
    });
    entries.sort();
    entries
}

fn is_glsl(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("glsl"))
}

/// A title's own choices: group id → preset id ("" for off).
pub type Choices = BTreeMap<String, String>;

/// Stored as `group=preset` lines (folder names can't hold a newline).
pub fn encode_choices(choices: &Choices) -> String {
    choices
        .iter()
        .map(|(group, preset)| format!("{group}={preset}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn decode_choices(text: &str) -> Choices {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(group, preset)| (group.to_string(), preset.to_string()))
        .collect()
}

/// The preset in effect for each group shown in the player: the title's
/// choice, else the group's default; `None` when off.
pub fn active<'a>(
    groups: &'a [Group],
    settings: &ShaderSettings,
    title: &Choices,
) -> Vec<(&'a Group, Option<&'a Preset>)> {
    groups
        .iter()
        .filter(|group| settings.shown(&group.id))
        .map(|group| {
            let wanted = title
                .get(&group.id)
                .or_else(|| settings.defaults.get(&group.id))
                .map(String::as_str)
                .unwrap_or("");
            (group, group.presets.iter().find(|p| p.id == wanted))
        })
        .collect()
}

/// Whether shader text has a `//!COMPUTE` directive (compute hooks).
fn has_compute(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().starts_with("//!COMPUTE"))
}

/// True for a user shader with compute hooks, which this player can't run
/// (they make mpv abort). Warns once per file.
fn is_unsupported_compute(path: &Path) -> bool {
    static WARNED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    if !has_compute(&text) {
        return false;
    }
    if WARNED
        .lock()
        .is_ok_and(|mut warned| warned.insert(path.to_path_buf()))
    {
        tracing::warn!(
            "skipping shader {}: it uses //!COMPUTE and compute shaders aren't supported in this player",
            path.display()
        );
    }
    true
}

/// The files to hand mpv, in the order mpv runs their hook stages: chroma
/// and luma prescalers (CHROMA/LUMA), Anime4K (MAIN), custom groups, FSR
/// (the last upscale to the screen), then sharpening (CAS sharpens FSR's
/// output at 1:1). Custom files with compute hooks are left out. With
/// `luma_always`, the luma group's files come from the ungated copies in
/// `luma-always/` (same file names).
pub fn chain(active: &[(&Group, Option<&Preset>)], luma_always: bool) -> Vec<PathBuf> {
    let rank = |group: &Group| match group.id.as_str() {
        CHROMA => 0,
        LUMA => 1,
        ANIME4K => 2,
        FSR => 4,
        SHARPEN => 5,
        _ => 3,
    };
    let mut on: Vec<(&Group, &Preset)> = active
        .iter()
        .filter_map(|(group, preset)| preset.map(|p| (*group, p)))
        .collect();
    on.sort_by_key(|(group, _)| rank(group));
    on.into_iter()
        .flat_map(|(group, preset)| {
            let custom = group.id.starts_with(CUSTOM_PREFIX);
            preset
                .files
                .iter()
                .filter(move |file| !(custom && is_unsupported_compute(file)))
                .map(move |file| {
                    let ungated = luma_always && group.id == LUMA;
                    match file.parent().and_then(Path::parent).zip(file.file_name()) {
                        Some((root, name)) if ungated => root.join(LUMA_ALWAYS).join(name),
                        _ => file.clone(),
                    }
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_groups_from_folders() {
        let root = std::env::temp_dir().join(format!("ecp-shaders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let touch = |path: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "//!HOOK MAIN").unwrap();
        };
        touch("Mine/Sharp.glsl");
        touch("Mine/Soft/2-upscale.glsl");
        touch("Mine/Soft/1-denoise.glsl");
        touch("Mine/readme.txt");
        touch("empty/notes.txt");
        let groups = custom_groups(&root);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "custom:Mine");
        let names: Vec<&str> = groups[0].presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Sharp", "Soft"]);
        let soft: Vec<String> = groups[0].presets[1]
            .files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(soft, ["1-denoise.glsl", "2-upscale.glsl"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fsr_presets_change_only_sharpness() {
        let sharp = fsr_with("0.0");
        assert!(sharp.contains("#define SHARPNESS 0.0 "));
        assert!(!sharp.contains("#define SHARPNESS 0.2 "));
        assert_eq!(sharp.len(), FSR_SOURCE.len());
    }

    #[test]
    fn choices_round_trip() {
        let mut choices = Choices::new();
        choices.insert("anime4k".into(), "a-fast".into());
        choices.insert("custom:My=Odd".into(), String::new());
        assert_eq!(decode_choices(&encode_choices(&choices)).len(), 2);
        assert_eq!(
            decode_choices("anime4k=a-fast\nfsr=").get("fsr"),
            Some(&String::new())
        );
    }

    #[test]
    fn title_choice_beats_default_and_groups_stack_in_order() {
        let groups = builtin_groups(Path::new("/s"));
        let mut settings = ShaderSettings::default();
        settings.defaults.insert(FSR.into(), "soft".into());
        settings.defaults.insert(ANIME4K.into(), "a-hq".into());
        let mut title = Choices::new();
        title.insert(ANIME4K.into(), "c-fast".into());
        let active = active(&groups, &settings, &title);
        let files = chain(&active, false);
        assert!(files[0].ends_with("anime4k/Anime4K_Clamp_Highlights.glsl"));
        assert!(files[1].ends_with("Anime4K_Upscale_Denoise_CNN_x2_M.glsl"));
        assert!(files.last().unwrap().ends_with("fsr/FSR-soft.glsl"));
        // Explicit off for this title.
        title.insert(FSR.into(), String::new());
        assert!(
            !chain(&super::active(&groups, &settings, &title), false)
                .iter()
                .any(|f| f.to_string_lossy().contains("FSR"))
        );
        // A hidden group is never applied.
        settings.hidden.push(ANIME4K.into());
        assert_eq!(
            chain(&super::active(&groups, &settings, &title), false),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn bundled_groups_use_bundled_files_and_chain_by_hook_stage() {
        let groups = builtin_groups(Path::new("/s"));
        for group in groups
            .iter()
            .filter(|g| [CHROMA, LUMA, SHARPEN].contains(&g.id.as_str()))
        {
            for file in group.presets.iter().flat_map(|p| p.files.iter()) {
                let name = file.file_name().unwrap().to_string_lossy();
                assert!(
                    BUNDLED_FILES
                        .iter()
                        .flat_map(|f| f.iter())
                        .any(|(folder, n, _)| *folder == group.id && *n == name),
                    "{name} is not bundled"
                );
            }
        }
        let mut settings = ShaderSettings::default();
        for (group, preset) in [
            (SHARPEN, "cas"),
            (FSR, "soft"),
            (ANIME4K, "a-fast"),
            (LUMA, "ravu-lite-ar"),
            (CHROMA, "krig"),
        ] {
            settings.defaults.insert(group.into(), preset.into());
        }
        let files = chain(&active(&groups, &settings, &Choices::new()), false);
        assert!(files[0].ends_with("chroma/KrigBilateral.glsl"));
        assert!(files[1].ends_with("luma/ravu-lite-ar-r4.hook"));
        assert!(files[2].ends_with("anime4k/Anime4K_Clamp_Highlights.glsl"));
        assert!(files.last().unwrap().ends_with("sharpen/CAS-scaled.glsl"));
        assert!(hint(LUMA).is_some() && hint("custom:x").is_none());
    }

    #[test]
    fn compute_shaders_are_detected_and_skipped_for_custom_groups() {
        assert!(has_compute("//!DESC x\n//!COMPUTE 32 8\n//!BIND HOOKED\n"));
        assert!(has_compute("  //!COMPUTE 8 8"));
        assert!(!has_compute("//!HOOK LUMA\n//!BIND HOOKED\n// COMPUTE\n"));

        let root = std::env::temp_dir().join(format!("ecp-compute-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let bad = root.join("bad.glsl");
        let good = root.join("good.glsl");
        std::fs::write(&bad, "//!HOOK LUMA\n//!COMPUTE 32 8\n").unwrap();
        std::fs::write(&good, "//!HOOK LUMA\n").unwrap();
        let group = Group {
            id: format!("{CUSTOM_PREFIX}x"),
            name: "x".into(),
            presets: vec![Preset {
                id: "p".into(),
                name: "p".into(),
                files: vec![bad.clone(), good.clone()],
            }],
        };
        let files = chain(&[(&group, Some(&group.presets[0]))], false);
        assert_eq!(files, vec![good]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn strip_when_drops_only_the_gates() {
        let text = "//!HOOK LUMA\n//!WHEN HOOKED.h OUTPUT.h / 0.7 <\n  //!WHEN a b >\n//!BIND HOOKED\nvoid x() {}\n";
        assert_eq!(
            strip_when(text),
            "//!HOOK LUMA\n//!BIND HOOKED\nvoid x() {}\n"
        );
        for (_, name, text) in BUNDLED_FILES
            .iter()
            .flat_map(|f| f.iter())
            .filter(|(d, n, _)| *d == "luma" && *n != "LICENSE")
        {
            assert!(!strip_when(text).contains("//!WHEN"), "{name}");
        }
    }

    #[test]
    fn luma_always_swaps_only_the_luma_folder() {
        let mk = |id: &str, file: &str| Group {
            id: id.into(),
            name: id.into(),
            presets: vec![Preset {
                id: "p".into(),
                name: "p".into(),
                files: vec![PathBuf::from(format!("/c/shaders/{id}/{file}"))],
            }],
        };
        let (luma, chroma) = (mk(LUMA, "a.hook"), mk(CHROMA, "b.glsl"));
        let active = [
            (&chroma, Some(&chroma.presets[0])),
            (&luma, Some(&luma.presets[0])),
        ];
        assert_eq!(
            chain(&active, true),
            vec![
                PathBuf::from("/c/shaders/chroma/b.glsl"),
                PathBuf::from("/c/shaders/luma-always/a.hook")
            ]
        );
        assert_eq!(
            chain(&active, false)[1],
            PathBuf::from("/c/shaders/luma/a.hook")
        );
    }
}
