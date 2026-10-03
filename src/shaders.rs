//! GLSL shader presets for mpv: the bundled FSR and Anime4K, plus the
//! user's own from `~/.config/embyclientplus/shaders/<group>/`. Each group
//! has at most one preset active; active presets of different groups stack.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::ShaderSettings;

pub const FSR: &str = "fsr";
pub const ANIME4K: &str = "anime4k";
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

/// Where the user's own shader groups live.
pub fn custom_dir() -> Option<PathBuf> {
    crate::config::config_dir()
        .ok()
        .map(|dir| dir.join("shaders"))
}

fn builtin_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "krakerz", "embyclientplus")
        .map(|dirs| dirs.cache_dir().join("shaders"))
}

/// Every group, in menu order: FSR, Anime4K, then custom folders by name.
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
    Ok(())
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
    vec![
        fsr,
        Group {
            id: ANIME4K.into(),
            name: "Anime4K".into(),
            presets: anime4k,
        },
    ]
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

/// The files to hand mpv, in order: Anime4K first (it works on the
/// source picture), then custom groups, then FSR (the last upscale to the
/// screen).
pub fn chain(active: &[(&Group, Option<&Preset>)]) -> Vec<PathBuf> {
    let rank = |group: &Group| match group.id.as_str() {
        ANIME4K => 0,
        FSR => 2,
        _ => 1,
    };
    let mut on: Vec<(&Group, &Preset)> = active
        .iter()
        .filter_map(|(group, preset)| preset.map(|p| (*group, p)))
        .collect();
    on.sort_by_key(|(group, _)| rank(group));
    on.into_iter()
        .flat_map(|(_, preset)| preset.files.iter().cloned())
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
        let files = chain(&active);
        assert!(files[0].ends_with("anime4k/Anime4K_Clamp_Highlights.glsl"));
        assert!(files[1].ends_with("Anime4K_Upscale_Denoise_CNN_x2_M.glsl"));
        assert!(files.last().unwrap().ends_with("fsr/FSR-soft.glsl"));
        // Explicit off for this title.
        title.insert(FSR.into(), String::new());
        assert!(
            !chain(&super::active(&groups, &settings, &title))
                .iter()
                .any(|f| f.to_string_lossy().contains("FSR"))
        );
        // A hidden group is never applied.
        settings.hidden.push(ANIME4K.into());
        assert_eq!(
            chain(&super::active(&groups, &settings, &title)),
            Vec::<PathBuf>::new()
        );
    }
}
