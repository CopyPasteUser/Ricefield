mod app;
mod clipboard;
mod config;
mod filesystem;
mod journal;
mod operation_queue;
mod preview;
mod theme;

use std::{env, ffi::OsString, path::PathBuf};

fn location_argument(argument: OsString) -> Result<PathBuf, &'static str> {
    let Some(text) = argument.to_str() else {
        // Ordinary Linux paths may contain non-UTF-8 bytes.
        return Ok(PathBuf::from(argument));
    };
    if text
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        return local_file_uri(text);
    }
    if looks_like_uri(text) {
        return Err("Remote URIs are not yet supported; open a locally mounted path instead");
    }
    Ok(PathBuf::from(argument))
}

fn looks_like_uri(text: &str) -> bool {
    let Some((scheme, suffix)) = text.split_once(':') else {
        return false;
    };
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        && (suffix.starts_with('/') || matches!(scheme, "trash" | "recent" | "network"))
}

fn local_file_uri(uri: &str) -> Result<PathBuf, &'static str> {
    let rest = uri.get(7..).ok_or("Invalid file URI")?;
    let absolute = if rest.starts_with('/') {
        rest.to_owned()
    } else {
        let (host, path) = rest.split_once('/').ok_or("Invalid file URI")?;
        if !host.eq_ignore_ascii_case("localhost") {
            return Err("Only local file:// URIs are supported");
        }
        format!("/{path}")
    };
    if absolute.contains(&['?', '#'][..]) {
        return Err("Invalid file URI: query and fragment are not supported");
    }
    let source = absolute.as_bytes();
    let mut decoded = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        if source[index] == b'%' {
            let encoded = source
                .get(index + 1..index + 3)
                .ok_or("Invalid file URI escape")?;
            let hex = std::str::from_utf8(encoded).map_err(|_| "Invalid file URI escape")?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| "Invalid file URI escape")?);
            index += 3;
        } else {
            decoded.push(source[index]);
            index += 1;
        }
    }
    if decoded.contains(&0) {
        return Err("Invalid file URI: NUL byte");
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(decoded)))
    }
    #[cfg(not(unix))]
    {
        let path = String::from_utf8(decoded).map_err(|_| "Invalid file URI encoding")?;
        Ok(PathBuf::from(path))
    }
}

fn invalid_argument(message: &str) -> ! {
    eprintln!("rice-explorer: {message}");
    std::process::exit(2)
}

fn main() -> eframe::Result {
    let mut selected_theme = None;
    let mut theme_file = None;
    let mut locations = Vec::<PathBuf>::new();
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "RiceExplorer {}\nUsage: rice-explorer [--theme ID] [--theme-file FILE] [FOLDER ...]\n\n--version        Show the package version\n--list-themes    Show bundled profiles\n--theme ID       Select a RiceBowl visual profile\n--theme-file FILE  Load an external editable TOML theme\n\nAccepts local paths and file:// URIs. Remote URIs are not yet supported.\n\nKeyboard: Ctrl+L path, Ctrl+F search, Ctrl+T/W tabs, Ctrl+C/X/V copy/cut/paste, F2 rename, Delete trash, Shift+Delete permanent delete, Ctrl+A select all, Ctrl+D duplicate, Alt+Enter properties, Space preview, Ctrl+H hidden, Ctrl+R refresh.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            Some("--version" | "-V") => {
                println!("RiceExplorer {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--list-themes") => {
                for id in theme::BUILTIN_IDS {
                    println!("{id}");
                }
                return Ok(());
            }
            Some("--theme") => {
                let theme = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .unwrap_or_else(|| invalid_argument("--theme requires a UTF-8 theme ID"));
                selected_theme = Some(theme);
            }
            Some("--theme-file") => {
                let path = args
                    .next()
                    .unwrap_or_else(|| invalid_argument("--theme-file requires a path"));
                theme_file = Some(PathBuf::from(path));
            }
            Some("--") => {
                for location in args.by_ref() {
                    let path =
                        location_argument(location).unwrap_or_else(|error| invalid_argument(error));
                    locations.push(path);
                }
            }
            Some(value) if value.starts_with('-') => {
                invalid_argument("Unknown option; see --help");
            }
            _ => {
                let path = location_argument(arg).unwrap_or_else(|error| invalid_argument(error));
                locations.push(path);
            }
        }
    }
    let icon = image::load_from_memory(include_bytes!("../assets/rice-explorer.png"))
        .expect("embedded PNG icon")
        .to_rgba8();
    let (width, height) = icon.dimensions();
    let icon = eframe::egui::IconData {
        rgba: icon.into_raw(),
        width,
        height,
    };
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([680.0, 440.0])
            .with_icon(std::sync::Arc::new(icon)),
        ..Default::default()
    };
    eframe::run_native(
        "RiceExplorer",
        options,
        Box::new(move |creation| {
            Ok(Box::new(app::Explorer::new(
                &creation.egui_ctx,
                selected_theme,
                theme_file,
                locations,
            )))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_file_uris_preserve_paths() {
        assert_eq!(
            location_argument("file:///tmp/a%20b%23c".into()).unwrap(),
            PathBuf::from("/tmp/a b#c")
        );
        assert_eq!(
            location_argument("file://LOCALHOST/home/user".into()).unwrap(),
            PathBuf::from("/home/user")
        );
        assert_eq!(
            location_argument("FILE:///tmp/README".into()).unwrap(),
            PathBuf::from("/tmp/README")
        );
        assert_eq!(
            location_argument("notes:2026".into()).unwrap(),
            PathBuf::from("notes:2026")
        );
    }

    #[test]
    fn invalid_or_remote_uris_are_rejected() {
        for uri in [
            "smb://server/share",
            "sftp://example.org/home",
            "davs://example.org/drive",
            "trash:///",
            "file://remote-host/home",
            "file:///tmp/a%XXb",
            "file:///tmp/a%00b",
            "file:///tmp/name#fragment",
        ] {
            assert!(location_argument(uri.into()).is_err(), "accepted: {uri}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_non_utf8_names_survive_argument_parsing() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let raw = OsString::from_vec(b"/tmp/\xffname".to_vec());
        assert_eq!(
            location_argument(raw).unwrap().as_os_str().as_bytes(),
            b"/tmp/\xffname"
        );
        assert_eq!(
            location_argument("file:///tmp/%FFname".into())
                .unwrap()
                .as_os_str()
                .as_bytes(),
            b"/tmp/\xffname"
        );
    }
}
