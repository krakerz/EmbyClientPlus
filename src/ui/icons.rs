//! The app's icon set (Lucide, converted to fills; see
//! scripts/update-icons.sh). Installed at startup as an icon theme that
//! inherits Adwaita, so it also replaces the icons GTK and libadwaita use
//! internally (back arrow, window buttons, dropdown arrows, ...).

use std::path::PathBuf;

use gtk::gdk;

include!(concat!(env!("OUT_DIR"), "/icons.rs"));

const THEME: &str = "EmbyClientPlus";

pub const BACK: &str = "ecp-back-symbolic";
pub const NEXT: &str = "ecp-next-symbolic";
pub const HOME: &str = "ecp-home-symbolic";
pub const SEARCH: &str = "ecp-search-symbolic";
pub const MENU: &str = "ecp-menu-symbolic";
pub const WATCHED: &str = "ecp-watched-symbolic";
pub const FAVORITE: &str = "ecp-favorite-symbolic";
pub const NOT_FAVORITE: &str = "ecp-not-favorite-symbolic";
pub const MOVIES: &str = "ecp-movies-symbolic";
pub const SHOWS: &str = "ecp-shows-symbolic";
pub const SUGGESTIONS: &str = "ecp-suggestions-symbolic";
pub const COLLECTIONS: &str = "ecp-collections-symbolic";
pub const GENRES: &str = "ecp-genres-symbolic";
pub const TAGS: &str = "ecp-tags-symbolic";
pub const FOLDER: &str = "ecp-folder-symbolic";
pub const PLAY: &str = "ecp-play-symbolic";
pub const PAUSE: &str = "ecp-pause-symbolic";
pub const SKIP_FORWARD: &str = "ecp-skip-forward-symbolic";
pub const SKIP_BACK: &str = "ecp-skip-back-symbolic";
pub const VOLUME: &str = "ecp-volume-symbolic";
pub const MUTED: &str = "ecp-muted-symbolic";
pub const AUDIO: &str = "ecp-audio-symbolic";
pub const SUBTITLES: &str = "ecp-subtitles-symbolic";
pub const QUALITY: &str = "ecp-quality-symbolic";
pub const FULLSCREEN: &str = "ecp-fullscreen-symbolic";
pub const UNFULLSCREEN: &str = "ecp-unfullscreen-symbolic";
pub const SORT_ASCENDING: &str = "ecp-sort-ascending-symbolic";
pub const SORT_DESCENDING: &str = "ecp-sort-descending-symbolic";
pub const RESET: &str = "ecp-reset-symbolic";
pub const CONTROLLER: &str = "ecp-controller-symbolic";
pub const SETTINGS: &str = "ecp-settings-symbolic";
pub const ALBUMS: &str = "ecp-albums-symbolic";
pub const ARTISTS: &str = "ecp-artists-symbolic";
pub const SONGS: &str = "ecp-songs-symbolic";
pub const EPISODE: &str = "ecp-episode-symbolic";
pub const PERSON: &str = "ecp-person-symbolic";
pub const PLAYLIST: &str = "ecp-playlist-symbolic";
pub const IMAGE: &str = "ecp-image-symbolic";
pub const PICTURE: &str = "ecp-picture-symbolic";
pub const SHADERS: &str = "ecp-shaders-symbolic";
pub const SVP: &str = "ecp-svp-symbolic";
pub const KEYBOARD: &str = "ecp-keyboard-symbolic";
pub const DOWNLOAD: &str = "ecp-download-symbolic";
pub const HISTORY: &str = "ecp-history-symbolic";
pub const SHUFFLE: &str = "ecp-shuffle-symbolic";
pub const REPEAT: &str = "ecp-repeat-symbolic";
pub const REPEAT_ONE: &str = "ecp-repeat-one-symbolic";
pub const MOVE_UP: &str = "ecp-move-up-symbolic";
pub const MOVE_DOWN: &str = "ecp-move-down-symbolic";
pub const REMOVE: &str = "ecp-remove-symbolic";
pub const COLLAPSE: &str = "ecp-collapse-symbolic";

/// The placeholder for an item that has no artwork.
pub fn placeholder(item_type: &str) -> &'static str {
    match item_type {
        "MusicAlbum" | "Audio" => SONGS,
        "Movie" | "Trailer" | "Video" => MOVIES,
        "Series" | "Season" => SHOWS,
        "Episode" => EPISODE,
        "Folder" | "CollectionFolder" | "BoxSet" | "UserView" => FOLDER,
        "MusicArtist" => ARTISTS,
        "Person" => PERSON,
        "Playlist" => PLAYLIST,
        _ => IMAGE,
    }
}

/// Writes the icons out as a theme and makes it the app's icon theme.
/// Failures only cost the custom look; Adwaita's icons remain.
pub fn install() {
    let Some(root) = crate::config::cache_dir().map(|dir| dir.join("icons")) else {
        return;
    };
    if let Err(e) = write_theme(&root) {
        tracing::warn!("couldn't install the icon theme: {e}");
        return;
    }
    if let Some(display) = gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(&root);
    }
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_icon_theme_name(Some(THEME));
    }
}

/// The app's own icon (the AppImage/desktop one), for the login page and
/// the window when the app isn't installed.
pub const APP: &str = "io.github.krakerz.EmbyClientPlus";
const APP_SVG: &[u8] = include_bytes!("../../packaging/io.github.krakerz.EmbyClientPlus.svg");

fn write_theme(root: &std::path::Path) -> std::io::Result<()> {
    let theme = root.join(THEME);
    let icons: PathBuf = theme.join("scalable/actions");
    let apps: PathBuf = theme.join("scalable/apps");
    std::fs::create_dir_all(&icons)?;
    std::fs::create_dir_all(&apps)?;
    std::fs::write(
        theme.join("index.theme"),
        format!(
            "[Icon Theme]\nName={THEME}\nComment=Emby Client+ icons (Lucide)\nInherits=Adwaita,hicolor\nDirectories=scalable/actions,scalable/apps\n\n[scalable/actions]\nSize=16\nMinSize=8\nMaxSize=512\nType=Scalable\nContext=Actions\n\n[scalable/apps]\nSize=128\nMinSize=16\nMaxSize=512\nType=Scalable\nContext=Applications\n"
        ),
    )?;
    let files = ICONS
        .iter()
        .map(|(name, svg)| (icons.join(format!("{name}.svg")), *svg))
        .chain([(apps.join(format!("{APP}.svg")), APP_SVG)]);
    for (path, svg) in files {
        // Skip unchanged files so the theme's cache stays valid.
        if std::fs::read(&path).ok().as_deref() != Some(svg) {
            std::fs::write(path, svg)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ICONS;

    #[test]
    fn placeholders_cover_each_type() {
        assert_eq!(super::placeholder("MusicAlbum"), super::SONGS);
        assert_eq!(super::placeholder("Episode"), super::EPISODE);
        assert_eq!(super::placeholder("Photo"), super::IMAGE);
        for kind in [
            "Movie",
            "Series",
            "Folder",
            "MusicArtist",
            "Person",
            "Playlist",
            "?",
        ] {
            let name = super::placeholder(kind);
            assert!(
                ICONS.iter().any(|(icon, _)| *icon == name),
                "{name} missing"
            );
        }
    }

    #[test]
    fn every_constant_has_an_embedded_icon() {
        for name in [
            super::BACK,
            super::HOME,
            super::FAVORITE,
            super::CONTROLLER,
            super::SUBTITLES,
            super::FOLDER,
            super::SORT_DESCENDING,
        ] {
            assert!(
                ICONS.iter().any(|(icon, _)| *icon == name),
                "{name} missing"
            );
        }
        assert!(
            ICONS
                .iter()
                .any(|(icon, _)| *icon == "window-close-symbolic")
        );
    }
}
