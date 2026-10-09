//! Preferences: global defaults for playback, plus housekeeping. Each
//! change is written to config.toml right away.

use adw::prelude::*;
use gtk::{gio, glib};

use crate::config::{
    DEFAULT_SVP_SOCKET, EpisodeArt, FrameGenBackend, FullscreenMode, Settings, Theme,
    TrailerQuality, VideoQuality,
};
use crate::controller::{self, Action, Context};
use crate::db::Db;
use crate::playback::tracks::normalize_language;
use crate::playback::{QUALITIES, Quality};

/// Languages offered in the pickers (ISO 639-2, display name).
const LANGUAGES: [(&str, &str); 16] = [
    ("eng", "English"),
    ("jpn", "Japanese"),
    ("zho", "Chinese"),
    ("kor", "Korean"),
    ("ind", "Indonesian"),
    ("msa", "Malay"),
    ("tha", "Thai"),
    ("vie", "Vietnamese"),
    ("spa", "Spanish"),
    ("por", "Portuguese"),
    ("fra", "French"),
    ("deu", "German"),
    ("ita", "Italian"),
    ("rus", "Russian"),
    ("ara", "Arabic"),
    ("nld", "Dutch"),
];

pub fn show(parent: &impl IsA<gtk::Widget>) {
    show_with(parent, true);
}

/// For the standalone player: no server, so none of its settings.
pub fn show_player_only(parent: &impl IsA<gtk::Widget>) {
    show_with(parent, false);
}

/// `server`: settings for an Emby server (streaming quality, Home) apply.
fn show_with(parent: &impl IsA<gtk::Widget>, server: bool) {
    let settings = Settings::load().unwrap_or_default();
    // A plain dialog rather than AdwPreferencesDialog, whose header has no
    // room for the Quit button (standalone player).
    let toasts = adw::ToastOverlay::new();
    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name(crate::ui::icons::SETTINGS)
        .build();

    // Playback.
    let playback = adw::PreferencesGroup::builder()
        .title("Playback")
        .description(
            "Defaults for new playback; the player's menus change them per title or session.",
        )
        .build();
    let svp = adw::SwitchRow::builder()
        .title("Use SVP by default")
        .subtitle("Titles where you toggled SVP in the player keep their own choice")
        .active(settings.frame_gen.default_backend == FrameGenBackend::Svp)
        .build();
    svp.connect_active_notify(|row| {
        let backend = if row.is_active() {
            FrameGenBackend::Svp
        } else {
            FrameGenBackend::Off
        };
        save(|s| s.frame_gen.default_backend = backend);
    });
    playback.add(&svp);
    let auto_start = adw::SwitchRow::builder()
        .title("Start SVP Manager automatically")
        .subtitle("When SVP is wanted and SVP Manager isn't running (on by default in Game Mode)")
        .active(settings.frame_gen.auto_start_svp())
        .build();
    auto_start.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.frame_gen.auto_start_svp = Some(on));
    });
    playback.add(&auto_start);
    let smooth = adw::SwitchRow::builder()
        .title("Smooth motion without SVP")
        .subtitle("mpv's frame blending, for titles where SVP is switched off")
        .active(settings.frame_gen.smooth_without_svp)
        .build();
    smooth.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.frame_gen.smooth_without_svp = on);
    });
    playback.add(&smooth);
    playback.add(&svp_folder_row(parent.upcast_ref()));
    playback.add(&svp_socket_row(&settings));

    let labels: Vec<String> = QUALITIES.iter().map(|q| q.label()).collect();
    let quality = adw::ComboRow::builder()
        .title("Default quality")
        .subtitle("Anything below Original asks the server to transcode")
        .model(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .build();
    let current = Quality::from_kbps(settings.playback.bitrate_cap_kbps());
    quality.set_selected(QUALITIES.iter().position(|q| *q == current).unwrap_or(0) as u32);
    quality.connect_selected_notify(|row| {
        if let Some(chosen) = QUALITIES.get(row.selected() as usize) {
            let kbps = chosen.kbps();
            save(|s| s.playback.set_bitrate_cap_kbps(kbps));
        }
    });
    if server {
        playback.add(&quality);
    }
    let mpv_conf = adw::SwitchRow::builder()
        .title("Use my mpv.conf")
        .subtitle("Loads ~/.config/mpv/mpv.conf (shaders, scalers, …) on the next start; the app's socket, output and decoding options still win")
        .active(settings.playback.use_mpv_conf)
        .build();
    mpv_conf.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.playback.use_mpv_conf = on);
    });
    playback.add(&mpv_conf);

    let trailer_labels: Vec<&str> = TrailerQuality::ALL.iter().map(|q| q.label()).collect();
    let trailer_quality = adw::ComboRow::builder()
        .title("Trailer quality")
        .subtitle("For web trailers (YouTube); needs yt-dlp")
        .model(&gtk::StringList::new(&trailer_labels))
        .build();
    trailer_quality.set_selected(
        TrailerQuality::ALL
            .iter()
            .position(|q| *q == settings.playback.trailer_quality)
            .unwrap_or(0) as u32,
    );
    trailer_quality.connect_selected_notify(|row| {
        if let Some(&chosen) = TrailerQuality::ALL.get(row.selected() as usize) {
            save(|s| s.playback.trailer_quality = chosen);
        }
    });
    playback.add(&trailer_quality);
    let trailer_captions = adw::SwitchRow::builder()
        .title("Trailer captions")
        .subtitle("Shows web trailers' captions in your subtitle language, when they have them")
        .active(settings.playback.trailer_captions)
        .build();
    trailer_captions.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.playback.trailer_captions = on);
    });
    playback.add(&trailer_captions);
    let keep_screen_on = adw::SwitchRow::builder()
        .title("Keep the screen on")
        .subtitle("While a video plays; music and paused videos let it sleep as usual")
        .active(settings.playback.keep_screen_on)
        .build();
    keep_screen_on.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.playback.keep_screen_on = on);
    });
    playback.add(&keep_screen_on);
    #[cfg(target_os = "linux")]
    playback.add(&do_not_disturb_row(&settings));
    let hide_paused = adw::SwitchRow::builder()
        .title("Hide player controls when paused")
        .subtitle("Otherwise they stay on screen while paused")
        .active(settings.playback.hide_osd_when_paused)
        .build();
    hide_paused.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.playback.hide_osd_when_paused = on);
    });
    playback.add(&hide_paused);
    let volume = adw::SpinRow::with_range(0.0, 130.0, 5.0);
    volume.set_title("Default volume");
    volume.set_subtitle(
        "For titles without their own; each title remembers its own, up to 130% (boost)",
    );
    volume.set_value(settings.playback.default_volume);
    volume.connect_value_notify(|row| {
        let value = row.value();
        save(|s| s.playback.default_volume = value);
    });
    playback.add(&volume);
    page.add(&playback);
    page.add(&downloads_group(parent.upcast_ref()));

    // Display.
    let display = adw::PreferencesGroup::builder()
        .title("Display")
        .description("Applies the next time the app starts")
        .build();
    let width = adw::SpinRow::with_range(640.0, 7680.0, 2.0);
    width.set_title("Window width");
    width.set_value(f64::from(settings.window.width));
    width.connect_value_notify(|row| {
        let value = row.value() as i32;
        save(|s| s.window.width = value);
    });
    let height = adw::SpinRow::with_range(400.0, 4320.0, 2.0);
    height.set_title("Window height");
    height.set_value(f64::from(settings.window.height));
    height.connect_value_notify(|row| {
        let value = row.value() as i32;
        save(|s| s.window.height = value);
    });
    let fullscreen = adw::ComboRow::builder()
        .title("Fullscreen")
        .subtitle("Keeps the whole app fullscreen, the player included")
        .model(&gtk::StringList::new(&[
            "Gamescope only",
            "Always",
            "Never",
        ]))
        .selected(match settings.window.fullscreen {
            FullscreenMode::GamescopeOnly => 0,
            FullscreenMode::Always => 1,
            FullscreenMode::Never => 2,
        })
        .build();
    fullscreen.connect_selected_notify(|row| {
        let mode = match row.selected() {
            1 => FullscreenMode::Always,
            2 => FullscreenMode::Never,
            _ => FullscreenMode::GamescopeOnly,
        };
        save(|s| s.window.fullscreen = mode);
        if let Some(window) = row.root().and_downcast::<gtk::Window>() {
            window.set_fullscreened(super::window::fullscreen_locked());
        }
    });
    let theme = adw::ComboRow::builder()
        .title("Theme")
        .model(&gtk::StringList::new(&["Follow system", "Light", "Dark"]))
        .selected(match settings.window.theme {
            Theme::System => 0,
            Theme::Light => 1,
            Theme::Dark => 2,
        })
        .build();
    theme.connect_selected_notify(|row| {
        let theme = match row.selected() {
            0 => Theme::System,
            1 => Theme::Light,
            _ => Theme::Dark,
        };
        super::window::apply_theme(theme);
        save(|s| s.window.theme = theme);
    });
    display.add(&theme);
    display.add(&width);
    display.add(&height);
    display.add(&fullscreen);
    page.add(&display);

    // Home.
    let home = adw::PreferencesGroup::builder().title("Home").build();
    let art = adw::ComboRow::builder()
        .title("Continue Watching artwork")
        .subtitle("Series art avoids spoilers from episode stills")
        .model(&gtk::StringList::new(&["Episode still", "Series art"]))
        .selected(match settings.home.episode_art {
            EpisodeArt::Episode => 0,
            EpisodeArt::Series => 1,
        })
        .build();
    art.connect_selected_notify(|row| {
        let art = if row.selected() == 1 {
            EpisodeArt::Series
        } else {
            EpisodeArt::Episode
        };
        save(|s| s.home.episode_art = art);
    });
    home.add(&art);
    if server {
        page.add(&home);
    }

    // Languages.
    let languages = adw::PreferencesGroup::builder()
        .title("Languages")
        .description("Used when a title has no remembered choice")
        .build();
    let audio = language_row("Audio", &settings.audio.preferred_language);
    audio.connect_selected_notify(|row| {
        if let Some(code) = selected_language(row) {
            save(|s| s.audio.preferred_language = code);
        }
    });
    let subtitles = language_row("Subtitles", &settings.subtitles.preferred_language);
    subtitles.connect_selected_notify(|row| {
        if let Some(code) = selected_language(row) {
            save(|s| s.subtitles.preferred_language = code);
        }
    });
    languages.add(&audio);
    languages.add(&subtitles);
    page.add(&languages);

    // Remembered choices.
    let remembered = adw::PreferencesGroup::builder()
        .title("Remembered Choices")
        .build();
    let forget = adw::ActionRow::builder()
        .title("Per-title choices")
        .subtitle("Audio, subtitle and SVP picks remembered for each series or movie")
        .build();
    let forget_button = gtk::Button::builder()
        .label("Forget All")
        .valign(gtk::Align::Center)
        .css_classes(["destructive-action"])
        .build();
    forget_button.connect_clicked(glib::clone!(
        #[weak]
        toasts,
        move |_| {
            let message = match Db::open_default().and_then(|db| db.clear_overrides()) {
                Ok(0) => "Nothing was remembered".to_string(),
                Ok(1) => "Forgot 1 title's choices".to_string(),
                Ok(n) => format!("Forgot {n} titles' choices"),
                Err(e) => {
                    tracing::warn!("{e:#}");
                    format!("Couldn't clear them: {e}")
                }
            };
            toasts.add_toast(
                adw::Toast::builder()
                    .title(glib::markup_escape_text(&message))
                    .timeout(crate::ui::TOAST_SECONDS)
                    .build(),
            );
        }
    ));
    forget.add_suffix(&forget_button);
    remembered.add(&forget);
    page.add(&remembered);

    if server {
        let parent = parent.as_ref().clone();
        let group = adw::PreferencesGroup::builder().title("Server").build();
        let browser = adw::SwitchRow::builder()
            .title("Appear as a web browser")
            .subtitle(
                "On: the server lists this app as a browser session. Off: as Emby Client+ \
                 on this computer, under its name.",
            )
            .active(settings.server.appear_as_browser())
            .build();
        browser.connect_active_notify(move |row| {
            let on = row.is_active();
            crate::emby::set_appear_as_browser(on);
            save(|s| s.server.appear_as_browser = Some(on));
            ask_sign_out(row, &parent);
        });
        group.add(&browser);
        page.add(&group);
    }

    // Diagnostics.
    let diagnostics = adw::PreferencesGroup::builder()
        .title("Diagnostics")
        .build();
    if let Some(logs) = crate::logging::log_dir() {
        let row = adw::ActionRow::builder()
            .title("Logs")
            .subtitle(logs.display().to_string())
            .subtitle_selectable(true)
            .build();
        let open = gtk::Button::builder()
            .label("Open Folder")
            .valign(gtk::Align::Center)
            .build();
        open.connect_clicked(move |_| {
            let uri = gio::File::for_path(&logs).uri();
            if let Err(e) =
                gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)
            {
                tracing::warn!("could not open {uri}: {e}");
            }
        });
        row.add_suffix(&open);
        diagnostics.add(&row);
    }
    page.add(&diagnostics);
    page.add(&super::updates::preferences_group(&toasts));

    let stack = adw::ViewStack::new();
    stack.add_titled_with_icon(
        &page,
        Some("general"),
        "General",
        crate::ui::icons::SETTINGS,
    );
    stack.add_titled_with_icon(
        &video_page(&settings),
        Some("video"),
        "Video",
        crate::ui::icons::PICTURE,
    );
    stack.add_titled_with_icon(
        &controller_page(),
        Some("controller"),
        "Controller",
        crate::ui::icons::CONTROLLER,
    );
    stack.add_titled_with_icon(
        &keyboard_page(),
        Some("keyboard"),
        "Keyboard",
        crate::ui::icons::KEYBOARD,
    );
    let header = adw::HeaderBar::builder()
        .title_widget(
            &adw::ViewSwitcher::builder()
                .stack(&stack)
                .policy(adw::ViewSwitcherPolicy::Wide)
                .build(),
        )
        .build();
    // Quitting from here is easier with a controller than Steam's menu.
    let quit = gtk::Button::builder()
        .label("Quit")
        .tooltip_text(format!("Quit {}", crate::APP_NAME))
        .css_classes(["destructive-action"])
        .build();
    let window = parent.as_ref().root().and_downcast::<gtk::Window>();
    quit.connect_clicked(move |_| {
        // Closing the window runs the usual shutdown: playback is reported
        // stopped and the SVP Manager we started is stopped.
        if let Some(window) = &window {
            super::window::shut_down(window);
        }
    });
    // The Emby app has Quit in Home's menu; the standalone player has none.
    if !server {
        header.pack_end(&quit);
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    toasts.set_child(Some(&toolbar));
    let dialog = adw::Dialog::builder()
        .title("Preferences")
        .content_width(780)
        .content_height(820)
        .child(&toasts)
        .build();
    dialog.present(Some(parent));
}

/// Where downloads are saved: typed (works in Game Mode, where file
/// choosers don't show) or picked with Choose… on the desktop.
fn downloads_group(parent: &gtk::Widget) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Downloads")
        .description(
            "Titles downloaded for offline playback (Download in a movie's or episode's menu)",
        )
        .build();
    let folder = adw::EntryRow::builder()
        .title("Download folder")
        .text(crate::downloads::folder().to_string_lossy())
        .show_apply_button(true)
        .build();
    folder.connect_apply(|row| {
        let text = row.text().trim().to_string();
        save(|s| s.downloads.folder = (!text.is_empty()).then_some(text));
        row.set_text(&crate::downloads::folder().to_string_lossy());
    });
    if !crate::gamescope::detected() {
        let choose = gtk::Button::builder()
            .label("Choose…")
            .valign(gtk::Align::Center)
            .build();
        let window = parent.root().and_downcast::<gtk::Window>();
        choose.connect_clicked(glib::clone!(
            #[weak]
            folder,
            move |_| {
                let dialog = gtk::FileDialog::builder()
                    .title("Choose the download folder")
                    .modal(true)
                    .build();
                dialog.set_initial_folder(Some(&gio::File::for_path(crate::downloads::folder())));
                dialog.select_folder(
                    window.as_ref(),
                    None::<&gio::Cancellable>,
                    glib::clone!(
                        #[weak]
                        folder,
                        move |result| {
                            let Some(path) = result.ok().and_then(|f| f.path()) else {
                                return; // cancelled
                            };
                            let value = path.to_string_lossy().into_owned();
                            folder.set_text(&value);
                            save(|s| s.downloads.folder = Some(value));
                        }
                    ),
                );
            }
        ));
        folder.add_suffix(&choose);
    }
    group.add(&folder);
    group
}

/// mpv scalers offered for the Custom quality: (mpv name, label).
const SCALERS: [(&str, &str); 7] = [
    ("bilinear", "Bilinear (cheapest)"),
    ("catmull_rom", "Bicubic (Catmull-Rom)"),
    ("mitchell", "Mitchell (soft)"),
    ("spline36", "Spline36"),
    ("lanczos", "Lanczos"),
    ("ewa_lanczos", "EWA Lanczos"),
    ("ewa_lanczossharp", "EWA Lanczos Sharp (heaviest)"),
];

/// Picture quality (scalers, deinterlacing, decoding) and shaders. Both
/// apply from the next video played.
fn video_page(settings: &Settings) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Video")
        .icon_name(crate::ui::icons::PICTURE)
        .build();

    let quality_group = adw::PreferencesGroup::builder()
        .title("Picture Quality")
        .description(
            "Applies from the next video. Heavier scalers cost more GPU, more so with SVP (every interpolated frame is scaled).",
        )
        .build();
    let labels: Vec<&str> = VideoQuality::ALL.iter().map(|q| q.label()).collect();
    let quality = adw::ComboRow::builder()
        .title("Quality preset")
        .subtitle("Fast suits the Steam Deck; High quality uses mpv's best scalers and debanding")
        .model(&gtk::StringList::new(&labels))
        .selected(
            VideoQuality::ALL
                .iter()
                .position(|q| *q == settings.video.quality)
                .unwrap_or(0) as u32,
        )
        .build();
    quality_group.add(&quality);

    let scaler_row = |title: &str, subtitle: &str, current: &str| {
        let labels: Vec<&str> = SCALERS.iter().map(|(_, label)| *label).collect();
        adw::ComboRow::builder()
            .title(title)
            .subtitle(subtitle)
            .model(&gtk::StringList::new(&labels))
            .selected(
                SCALERS
                    .iter()
                    .position(|(name, _)| *name == current)
                    .unwrap_or(3) as u32,
            )
            .build()
    };
    let upscaler = scaler_row(
        "Upscaler",
        "When the video is smaller than the screen",
        &settings.video.upscaler,
    );
    let chroma = scaler_row(
        "Chroma scaler",
        "Colour detail; subtle differences",
        &settings.video.chroma_scaler,
    );
    let downscaler = scaler_row(
        "Downscaler",
        "When the video is larger than the screen",
        &settings.video.downscaler,
    );
    let scaler_saved = |field: fn(&mut crate::config::VideoSettings) -> &mut String| {
        move |row: &adw::ComboRow| {
            if let Some((name, _)) = SCALERS.get(row.selected() as usize) {
                save(|s| *field(&mut s.video) = (*name).to_string());
            }
        }
    };
    upscaler.connect_selected_notify(scaler_saved(|v| &mut v.upscaler));
    chroma.connect_selected_notify(scaler_saved(|v| &mut v.chroma_scaler));
    downscaler.connect_selected_notify(scaler_saved(|v| &mut v.downscaler));
    let deband = adw::SwitchRow::builder()
        .title("Debanding")
        .subtitle("Smooths colour banding in gradients")
        .active(settings.video.deband)
        .build();
    deband.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.video.deband = on);
    });
    let custom_rows: Vec<gtk::Widget> = vec![
        upscaler.upcast(),
        chroma.upcast(),
        downscaler.upcast(),
        deband.upcast(),
    ];
    for row in &custom_rows {
        row.set_sensitive(settings.video.quality == VideoQuality::Custom);
        quality_group.add(row);
    }
    quality.connect_selected_notify(move |row| {
        let chosen = VideoQuality::ALL
            .get(row.selected() as usize)
            .copied()
            .unwrap_or_default();
        for row in &custom_rows {
            row.set_sensitive(chosen == VideoQuality::Custom);
        }
        save(|s| s.video.quality = chosen);
    });
    let deinterlace = adw::SwitchRow::builder()
        .title("Deinterlace")
        .subtitle("Only for videos marked as interlaced (DVDs, TV recordings)")
        .active(settings.video.deinterlace)
        .build();
    deinterlace.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.video.deinterlace = on);
    });
    quality_group.add(&deinterlace);
    let software = adw::SwitchRow::builder()
        .title("Software decoding")
        .subtitle("Decode on the CPU instead of the GPU; for videos the GPU decodes wrongly")
        .active(settings.video.software_decoding)
        .build();
    software.connect_active_notify(|row| {
        let on = row.is_active();
        save(|s| s.video.software_decoding = on);
    });
    quality_group.add(&software);
    page.add(&quality_group);

    let shader_group = adw::PreferencesGroup::builder()
        .title("Shaders")
        .description(
            "Shown groups get a submenu in the player's Shaders button, where each title keeps its own pick. One preset per group; presets of different groups stack. Upscalers only work when the video is smaller than the screen.",
        )
        .build();
    for group in crate::shaders::groups() {
        let shown = adw::SwitchRow::builder()
            .title(&group.name)
            .subtitle("Show in the player")
            .active(settings.shaders.shown(&group.id))
            .build();
        let mut labels = vec!["Off".to_string()];
        labels.extend(group.presets.iter().map(|p| p.name.clone()));
        let current = settings
            .shaders
            .defaults
            .get(&group.id)
            .and_then(|id| group.presets.iter().position(|p| &p.id == id))
            .map_or(0, |index| index + 1);
        let default = adw::ComboRow::builder()
            .title(format!("{} by default", group.name))
            .subtitle("For titles without their own pick")
            .model(&gtk::StringList::new(
                &labels.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
            .selected(current as u32)
            .sensitive(shown.is_active())
            .build();
        let group_id = group.id.clone();
        shown.connect_active_notify(glib::clone!(
            #[weak]
            default,
            move |row| {
                let on = row.is_active();
                default.set_sensitive(on);
                save(|s| {
                    s.shaders.hidden.retain(|g| g != &group_id);
                    if !on {
                        s.shaders.hidden.push(group_id.clone());
                    }
                });
            }
        ));
        let (group_id, presets) = (group.id.clone(), group.presets.clone());
        default.connect_selected_notify(move |row| {
            let preset = match row.selected() {
                0 => String::new(),
                n => presets
                    .get(n as usize - 1)
                    .map(|p| p.id.clone())
                    .unwrap_or_default(),
            };
            save(|s| {
                s.shaders.defaults.insert(group_id.clone(), preset);
            });
        });
        shader_group.add(&shown);
        shader_group.add(&default);
    }
    if let Some(dir) = crate::shaders::custom_dir() {
        let row = adw::ActionRow::builder()
            .title("Your shaders")
            .subtitle(format!(
                "{}: one folder per group, each .glsl file (or folder of them) a preset. Read when Preferences opens.",
                dir.display()
            ))
            .build();
        let open = gtk::Button::builder()
            .label("Open Folder")
            .valign(gtk::Align::Center)
            .build();
        open.connect_clicked(move |_| {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!("could not create {}: {e}", dir.display());
            }
            let uri = gio::File::for_path(&dir).uri();
            if let Err(e) =
                gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)
            {
                tracing::warn!("could not open {uri}: {e}");
            }
        });
        row.add_suffix(&open);
        shader_group.add(&row);
    }
    page.add(&shader_group);
    page
}

/// How long "Press a button…" waits before giving up.
const CAPTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Controller status and remapping, one row per action.
fn controller_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Controller")
        .icon_name(crate::ui::icons::CONTROLLER)
        .build();
    let status = adw::PreferencesGroup::builder().title("Controller").build();
    let connected = adw::ActionRow::builder()
        .title(match controller::connected() {
            Some(name) => name,
            None => "No controller connected".to_string(),
        })
        .subtitle("In Steam Game Mode, use the \u{201c}Gamepad\u{201d} controller layout")
        .build();
    connected.add_prefix(&gtk::Image::from_icon_name(crate::ui::icons::CONTROLLER));
    let reset = gtk::Button::builder()
        .label("Reset to Defaults")
        .valign(gtk::Align::Center)
        .build();
    connected.add_suffix(&reset);
    status.add(&connected);
    page.add(&status);

    let mut rows: Vec<(Action, adw::ActionRow)> = Vec::new();
    for (title, context) in [
        ("While Browsing", Context::Browse),
        ("In the Player", Context::Player),
        ("In the Music Player", Context::Music),
    ] {
        let group = adw::PreferencesGroup::builder().title(title).build();
        for action in Action::ALL.into_iter().filter(|a| a.context() == context) {
            let row = adw::ActionRow::builder().title(action.label()).build();
            let change = gtk::Button::builder()
                .label("Change")
                .valign(gtk::Align::Center)
                .build();
            row.add_suffix(&change);
            group.add(&row);
            let target = row.downgrade();
            change.connect_clicked(move |_| {
                if let Some(row) = target.upgrade() {
                    capture_binding(action, &row);
                }
            });
            rows.push((action, row));
        }
        page.add(&group);
    }
    let rows = std::rc::Rc::new(rows);
    refresh_bindings(&rows);
    reset.connect_clicked({
        let rows = rows.clone();
        move |_| {
            controller::set_bindings(controller::Bindings::default());
            refresh_bindings(&rows);
        }
    });
    // Rows show the mapping as it changes (a remap can move a button off
    // another action).
    let refresh = glib::timeout_add_local(std::time::Duration::from_millis(300), {
        let rows = std::rc::Rc::downgrade(&rows);
        move || match rows.upgrade() {
            Some(rows) => {
                refresh_bindings(&rows);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        }
    });
    let refresh = std::cell::RefCell::new(Some(refresh));
    page.connect_unmap(move |_| {
        if let Some(source) = refresh.take() {
            source.remove();
        }
    });
    page
}

fn refresh_bindings(rows: &[(Action, adw::ActionRow)]) {
    let bindings = controller::bindings();
    for (action, row) in rows {
        if row.subtitle().is_some_and(|s| s.starts_with("Press")) {
            continue; // capture in progress
        }
        let buttons = bindings.buttons(*action);
        let text = if buttons.is_empty() {
            "Not set".to_string()
        } else {
            buttons
                .iter()
                .map(|pad| pad.label())
                .collect::<Vec<_>>()
                .join(", ")
        };
        row.set_subtitle(&glib::markup_escape_text(&text));
    }
}

/// "Press a button…": the next controller press becomes `action`'s.
fn capture_binding(action: Action, row: &adw::ActionRow) {
    row.set_subtitle("Press a button on the controller…");
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    controller::capture_next({
        let done = done.clone();
        let row = row.downgrade();
        move |pad| {
            done.set(true);
            let mut bindings = controller::bindings();
            bindings.set(action, pad);
            controller::set_bindings(bindings);
            if let Some(row) = row.upgrade() {
                row.set_subtitle(&glib::markup_escape_text(pad.label()));
            }
        }
    });
    let row = row.downgrade();
    glib::timeout_add_local_once(CAPTURE_TIMEOUT, move || {
        if !done.get() {
            controller::cancel_capture();
            if let Some(row) = row.upgrade() {
                row.set_subtitle("No button pressed");
            }
        }
    });
}

/// Player keyboard shortcuts, one row per action.
fn keyboard_page() -> adw::PreferencesPage {
    use crate::keys::KeyAction;
    let page = adw::PreferencesPage::builder()
        .title("Keyboard")
        .icon_name(crate::ui::icons::KEYBOARD)
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("In the Player")
        .description(
            "Change a shortcut, then press the new key. A key moves off any other action using it.",
        )
        .build();
    let reset = gtk::Button::builder()
        .label("Reset to Defaults")
        .valign(gtk::Align::Center)
        .build();
    group.set_header_suffix(Some(&reset));
    let mut rows: Vec<(KeyAction, adw::ActionRow)> = Vec::new();
    let mut buttons = Vec::new();
    for action in KeyAction::ALL {
        let row = adw::ActionRow::builder().title(action.label()).build();
        let change = gtk::Button::builder()
            .label("Change")
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&change);
        group.add(&row);
        rows.push((action, row));
        buttons.push((action, change));
    }
    page.add(&group);
    let rows = std::rc::Rc::new(rows);
    refresh_keys(&rows);
    for (action, change) in buttons {
        let rows = std::rc::Rc::downgrade(&rows);
        change.connect_clicked(move |button| {
            if let Some(rows) = rows.upgrade() {
                capture_key(action, button, &rows);
            }
        });
    }
    reset.connect_clicked({
        let rows = rows.clone();
        move |_| {
            crate::keys::set_bindings(crate::keys::KeyBindings::default());
            refresh_keys(&rows);
        }
    });
    page
}

fn refresh_keys(rows: &[(crate::keys::KeyAction, adw::ActionRow)]) {
    let bindings = crate::keys::bindings();
    for (action, row) in rows {
        let keys = bindings.keys(*action);
        let text = if keys.is_empty() {
            "Not set".to_string()
        } else {
            keys.iter()
                .map(|k| crate::keys::key_label(k))
                .collect::<Vec<_>>()
                .join(", ")
        };
        row.set_subtitle(&glib::markup_escape_text(&text));
    }
}

/// "Press a key…": the next key pressed becomes `action`'s shortcut.
fn capture_key(
    action: crate::keys::KeyAction,
    button: &gtk::Button,
    rows: &std::rc::Rc<Vec<(crate::keys::KeyAction, adw::ActionRow)>>,
) {
    // On the dialog: it's always on the way to the focused Change button.
    let Some(host) = button.ancestor(adw::Dialog::static_type()) else {
        return;
    };
    if let Some((_, row)) = rows.iter().find(|(a, _)| *a == action) {
        row.set_subtitle("Press a key…");
    }
    let keys = gtk::EventControllerKey::new();
    // Ahead of everything, so Escape or Space become shortcuts instead of
    // closing the dialog or pressing the button.
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let finish = {
        let (host, keys, done, rows) = (host.downgrade(), keys.clone(), done.clone(), rows.clone());
        move || {
            if done.replace(true) {
                return;
            }
            if let Some(host) = host.upgrade() {
                host.remove_controller(&keys);
            }
            refresh_keys(&rows);
        }
    };
    keys.connect_key_pressed({
        let finish = finish.clone();
        move |_, key, _, _| {
            if let Some(name) = crate::keys::key_name(key) {
                let mut bindings = crate::keys::bindings();
                bindings.set(action, &name);
                crate::keys::set_bindings(bindings);
            }
            // Removed after this handler returns, not from inside it.
            glib::idle_add_local_once(finish.clone());
            glib::Propagation::Stop
        }
    });
    host.add_controller(keys);
    glib::timeout_add_local_once(CAPTURE_TIMEOUT, finish);
}

/// "SVP folder" with Choose… and Reset. Only detection uses it: the
/// VapourSynth libraries are linked from the folder given at build time.
fn svp_folder_row(parent: &gtk::Widget) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title("SVP folder")
        .tooltip_text(
            "Where SVP 4 is installed, used to find and start SVP Manager. On a Mac, \
             the SVP 4 Mac app or the folder it's in (Applications).",
        )
        .build();
    let reset = gtk::Button::builder()
        .icon_name(crate::ui::icons::RESET)
        .tooltip_text(format!("Back to {}", crate::svp::default_dir_label()))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let choose = gtk::Button::builder()
        .label("Choose…")
        .valign(gtk::Align::Center)
        .build();
    row.add_suffix(&reset);
    row.add_suffix(&choose);
    refresh_svp_folder(&row, &reset);

    reset.connect_clicked(glib::clone!(
        #[weak]
        row,
        move |reset| {
            save(|s| s.frame_gen.svp_dir.clear());
            refresh_svp_folder(&row, reset);
        }
    ));
    let window = parent.root().and_downcast::<gtk::Window>();
    choose.connect_clicked(glib::clone!(
        #[weak]
        row,
        #[weak]
        reset,
        move |choose| {
            // Portal file choosers don't show up in gamescope; type it instead.
            if crate::gamescope::detected() {
                ask_svp_folder(choose, &row, &reset);
                return;
            }
            let dialog = gtk::FileDialog::builder()
                .title("Choose the SVP 4 folder")
                .modal(true)
                .build();
            if let Some(current) = crate::svp::install_dir().filter(|dir| dir.exists()) {
                dialog.set_initial_folder(Some(&gio::File::for_path(current)));
            }
            dialog.select_folder(
                window.as_ref(),
                None::<&gio::Cancellable>,
                glib::clone!(
                    #[weak]
                    row,
                    #[weak]
                    reset,
                    move |result| {
                        let Some(path) = result.ok().and_then(|folder| folder.path()) else {
                            return; // cancelled
                        };
                        let value = path.to_string_lossy().into_owned();
                        save(|s| s.frame_gen.svp_dir = value);
                        refresh_svp_folder(&row, &reset);
                    }
                ),
            );
        }
    ));
    row
}

/// An in-window prompt for the SVP folder path (no file chooser).
/// "Do not disturb while watching", with optional commands for desktops
/// the app can't switch by itself.
#[cfg(target_os = "linux")]
fn do_not_disturb_row(settings: &Settings) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder()
        .title("Do not disturb while watching")
        .subtitle(
            "Holds notifications back while a video is open. Works with KDE Plasma, GNOME, \
             caelestia, swaync, dunst and mako; for anything else, give the commands below",
        )
        .show_enable_switch(true)
        .enable_expansion(settings.playback.do_not_disturb)
        .build();
    row.connect_enable_expansion_notify(|row| {
        let on = row.enables_expansion();
        save(|s| s.playback.do_not_disturb = on);
    });
    for (title, value, setter) in [
        (
            "Command to turn it on",
            settings.playback.dnd_on_command.clone(),
            (|s: &mut Settings, v: String| s.playback.dnd_on_command = v)
                as fn(&mut Settings, String),
        ),
        (
            "Command to turn it off",
            settings.playback.dnd_off_command.clone(),
            |s: &mut Settings, v: String| s.playback.dnd_off_command = v,
        ),
    ] {
        let entry = adw::EntryRow::builder()
            .title(title)
            .text(value)
            .show_apply_button(true)
            .build();
        entry.connect_apply(move |entry| {
            let text = entry.text().trim().to_string();
            save(|s| setter(s, text));
        });
        row.add_row(&entry);
    }
    row
}

/// The server keeps the app name it saw at sign-in, so a new identity
/// only shows after signing in again: offer that now.
fn ask_sign_out(anchor: &adw::SwitchRow, home: &gtk::Widget) {
    let dialog = adw::AlertDialog::builder()
        .heading("Sign in again?")
        .body(
            "The server shows the new name after you sign in again. Sign out now? Downloads and \
             settings are kept.",
        )
        .default_response("later")
        .close_response("later")
        .build();
    dialog.add_responses(&[("later", "Later"), ("sign-out", "Sign Out")]);
    dialog.set_response_appearance("sign-out", adw::ResponseAppearance::Suggested);
    let home = home.downgrade();
    let anchor_weak = anchor.downgrade();
    dialog.connect_response(None, move |_, response| {
        if response != "sign-out" {
            return;
        }
        // Close Preferences, then use Home's own Log Out.
        if let Some(prefs) = anchor_weak
            .upgrade()
            .and_then(|row| row.ancestor(adw::Dialog::static_type()))
            .and_then(|w| w.downcast::<adw::Dialog>().ok())
        {
            prefs.close();
        }
        if let Some(home) = home.upgrade()
            && home.activate_action("home.logout", None).is_err()
        {
            tracing::warn!("no Log Out action here; sign out from Home's menu");
        }
    });
    dialog.present(Some(anchor));
}

fn ask_svp_folder(anchor: &gtk::Button, row: &adw::ActionRow, reset: &gtk::Button) {
    let current = crate::svp::install_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    let entry = gtk::Entry::builder()
        .text(current)
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading("SVP folder")
        .body("Where SVP 4 is installed")
        .extra_child(&entry)
        .default_response("save")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save")]);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.connect_response(
        None,
        glib::clone!(
            #[weak]
            row,
            #[weak]
            reset,
            #[weak]
            entry,
            move |_, response| {
                if response == "save" {
                    let value = entry.text().trim().to_string();
                    save(|s| s.frame_gen.svp_dir = value);
                    refresh_svp_folder(&row, &reset);
                }
            }
        ),
    );
    dialog.present(Some(anchor));
}

/// The IPC socket path SVP Manager connects to, for setups where SVP's
/// mpv.conf was changed from /tmp/mpvsocket. Applies from the next playback.
fn svp_socket_row(settings: &Settings) -> adw::EntryRow {
    let row = adw::EntryRow::builder()
        .title("SVP socket")
        .text(settings.frame_gen.socket())
        .show_apply_button(true)
        .tooltip_text(
            "Must match input-ipc-server in SVP's mpv.conf. Takes effect on the next playback.",
        )
        .build();
    let reset = gtk::Button::builder()
        .icon_name(crate::ui::icons::RESET)
        .tooltip_text(format!("Back to {DEFAULT_SVP_SOCKET}"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    row.add_suffix(&reset);
    reset.connect_clicked(glib::clone!(
        #[weak]
        row,
        move |_| {
            row.set_text(DEFAULT_SVP_SOCKET);
            save(|s| s.frame_gen.svp_socket = DEFAULT_SVP_SOCKET.to_string());
        }
    ));
    row.connect_apply(|row| {
        let socket = row.text().trim().to_string();
        save(|s| s.frame_gen.svp_socket = socket);
    });
    row
}

fn refresh_svp_folder(row: &adw::ActionRow, reset: &gtk::Button) {
    let custom = !Settings::load()
        .unwrap_or_default()
        .frame_gen
        .svp_dir
        .trim()
        .is_empty();
    reset.set_visible(custom);
    let subtitle = match crate::svp::install_dir() {
        Some(dir) => {
            let status = if crate::svp::is_install(&dir) {
                "SVP found"
            } else {
                "SVPManager not found here"
            };
            format!("{} — {status}", dir.display())
        }
        None => "No home folder".to_string(),
    };
    row.set_subtitle(&glib::markup_escape_text(&subtitle));
}

/// A language picker preselected on `current`; an unlisted configured
/// code is kept as an extra entry so opening the dialog never changes it.
fn language_row(title: &str, current: &str) -> adw::ComboRow {
    let current = normalize_language(current);
    let mut codes: Vec<String> = LANGUAGES.iter().map(|(code, _)| code.to_string()).collect();
    let mut names: Vec<String> = LANGUAGES.iter().map(|(_, name)| name.to_string()).collect();
    if !codes.contains(&current) {
        codes.push(current.clone());
        names.push(current.clone());
    }
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .build();
    row.set_selected(codes.iter().position(|c| *c == current).unwrap_or(0) as u32);
    // SAFETY: only ever stored and read as `Vec<String>` under this key.
    unsafe { row.set_data(CODES_KEY, codes) };
    row
}

const CODES_KEY: &str = "embyclientplus-language-codes";

fn selected_language(row: &adw::ComboRow) -> Option<String> {
    // SAFETY: see `language_row`.
    let codes = unsafe { row.data::<Vec<String>>(CODES_KEY)?.as_ref() };
    codes.get(row.selected() as usize).cloned()
}

fn save(change: impl FnOnce(&mut Settings)) {
    if let Err(e) = Settings::update(change) {
        tracing::warn!("could not save preferences: {e:#}");
    }
}
