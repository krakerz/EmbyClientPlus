//! The command line: `-v`/`--version`, `-h`/`--help`, or a file/URL to
//! play directly. Handled before anything else starts (no window, no log
//! file for `--version`).

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Start the Emby app.
    Run,
    /// Just the player (no Emby): play `target`, or wait for one.
    Player {
        target: Option<String>,
    },
    Version,
    Help,
    /// An option we don't know.
    Unknown(String),
    /// A second target (or anything after the first one).
    Unexpected(String),
}

fn is_option(arg: &str) -> bool {
    arg.starts_with('-') && arg.len() > 1
}

/// From the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Command {
    let mut args = args.into_iter();
    let Some(arg) = args.next() else {
        return Command::Run;
    };
    let target = match arg.as_str() {
        "--player" => match args.next() {
            None => None,
            Some(next) => match next.as_str() {
                "-v" | "-V" | "--version" => return Command::Version,
                "-h" | "--help" => return Command::Help,
                "--" => args.next(),
                _ if is_option(&next) => return Command::Unknown(next),
                _ => Some(next),
            },
        },
        // `--` ends options: a file whose name starts with a dash.
        "--" => match args.next() {
            Some(target) => Some(target),
            None => return Command::Run,
        },
        "-v" | "-V" | "--version" => return Command::Version,
        "-h" | "--help" => return Command::Help,
        _ if is_option(&arg) => return Command::Unknown(arg),
        _ => Some(arg),
    };
    match args.next() {
        Some(extra) => Command::Unexpected(extra),
        None => Command::Player { target },
    }
}

pub fn version() -> String {
    format!("{} {}", crate::APP_NAME, crate::update::current_version())
}

pub fn help() -> String {
    // The folder differs per OS; the Linux one is the fallback.
    let config = crate::config::config_dir().map_or_else(
        |_| "~/.config/embyclientplus/".into(),
        |dir| format!("{}", dir.display()),
    );
    format!(
        "\
{version} — a native Emby client with SVP frame interpolation.

Usage:
  embyclientplus                start the app
  embyclientplus --player [<file|url>]
                                just the player, no Emby: plays the file or
                                URL, or waits for one (drop it in the window,
                                or pick from your downloads); SVP, shaders and
                                picture options included, and the next video
                                in the folder plays after each one
  embyclientplus <file|url>     the same
  embyclientplus -- <file>      the same, for a file named like an option

Options:
  -h, --help                    show this help and exit
  -v, --version                 show the version and exit

Settings and logs live in {config}.",
        version = version()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Command {
        parse(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn options_and_targets() {
        assert_eq!(parse_args(&[]), Command::Run);
        assert_eq!(parse_args(&["-v"]), Command::Version);
        assert_eq!(parse_args(&["--version"]), Command::Version);
        assert_eq!(parse_args(&["-h"]), Command::Help);
        assert_eq!(parse_args(&["--help"]), Command::Help);
        assert_eq!(
            parse_args(&["movie.mkv"]),
            Command::Player {
                target: Some("movie.mkv".into())
            }
        );
        assert_eq!(
            parse_args(&["--", "-odd.mkv"]),
            Command::Player {
                target: Some("-odd.mkv".into())
            }
        );
        assert_eq!(parse_args(&["--nope"]), Command::Unknown("--nope".into()));
        assert_eq!(
            parse_args(&["--player", "clip.mp4"]),
            Command::Player {
                target: Some("clip.mp4".into())
            }
        );
        assert_eq!(parse_args(&["--player"]), Command::Player { target: None });
        assert_eq!(parse_args(&["--player", "--help"]), Command::Help);
        assert_eq!(parse_args(&["--player", "-h"]), Command::Help);
        assert_eq!(parse_args(&["--player", "-v"]), Command::Version);
        assert_eq!(parse_args(&["--player", "--version"]), Command::Version);
        assert_eq!(
            parse_args(&["--player", "--", "-odd.mkv"]),
            Command::Player {
                target: Some("-odd.mkv".into())
            }
        );
        assert_eq!(
            parse_args(&["--player", "--"]),
            Command::Player { target: None }
        );
        assert_eq!(
            parse_args(&["--player", "-x"]),
            Command::Unknown("-x".into())
        );
        // A lone "-" isn't an option (some tools mean stdin by it).
        assert_eq!(
            parse_args(&["-"]),
            Command::Player {
                target: Some("-".into())
            }
        );
    }

    #[test]
    fn extra_arguments_are_errors() {
        let unexpected = |extra: &str| Command::Unexpected(extra.into());
        assert_eq!(parse_args(&["a.mkv", "b.mkv"]), unexpected("b.mkv"));
        assert_eq!(parse_args(&["a.mkv", "--help"]), unexpected("--help"));
        assert_eq!(parse_args(&["--player", "a.mkv", "b"]), unexpected("b"));
        assert_eq!(parse_args(&["--", "a.mkv", "b"]), unexpected("b"));
        assert_eq!(parse_args(&["--player", "--", "a", "b"]), unexpected("b"));
        assert_eq!(
            parse_args(&["--player", "--", "--"]),
            Command::Player {
                target: Some("--".into())
            }
        );
    }

    #[test]
    fn help_names_the_options() {
        let help = help();
        for needle in [
            "--help",
            "--version",
            "--player",
            "<file|url>",
            crate::APP_NAME,
        ] {
            assert!(help.contains(needle), "{needle}");
        }
        if let Ok(dir) = crate::config::config_dir() {
            assert!(help.contains(&dir.display().to_string()));
        }
    }
}
