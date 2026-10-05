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
}

/// From the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Command {
    let mut args = args.into_iter();
    match args.next() {
        None => Command::Run,
        Some(arg) => match arg.as_str() {
            "-v" | "-V" | "--version" => Command::Version,
            "-h" | "--help" => Command::Help,
            "--player" => Command::Player {
                target: args.next(),
            },
            // `--` ends options: a file whose name starts with a dash.
            "--" => match args.next() {
                Some(target) => Command::Player {
                    target: Some(target),
                },
                None => Command::Run,
            },
            option if option.starts_with('-') && option.len() > 1 => {
                Command::Unknown(option.to_string())
            }
            _ => Command::Player { target: Some(arg) },
        },
    }
}

pub fn version() -> String {
    format!("{} {}", crate::APP_NAME, crate::update::current_version())
}

pub fn help() -> String {
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

Settings and logs live in ~/.config/embyclientplus/.",
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
        // A lone "-" isn't an option (some tools mean stdin by it).
        assert_eq!(
            parse_args(&["-"]),
            Command::Player {
                target: Some("-".into())
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
    }
}
