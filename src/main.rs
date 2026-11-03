mod app;
mod config;
mod filesystem;
mod theme;

use std::{env, path::PathBuf};

fn main() -> eframe::Result {
    let mut selected_theme = None;
    let mut theme_file = None;
    let mut locations = Vec::<PathBuf>::new();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("RiceExplorer 0.1\nUsage: rice-explorer [--theme ID] [--theme-file FILE] [FOLDER ...]\n\n--list-themes    Show bundled profiles\n--theme ID       Select a RiceBowl visual profile\n--theme-file FILE  Load an external editable TOML theme\n\nKeyboard: Ctrl+L path, Ctrl+F search, Ctrl+T/W tabs, Ctrl+C/X/V copy/cut/paste, F2 rename, Delete trash, Ctrl+H hidden, Ctrl+R refresh.");
                return Ok(());
            }
            "--list-themes" => {
                for id in theme::BUILTIN_IDS { println!("{id}"); }
                return Ok(());
            }
            "--theme" => selected_theme = args.next(),
            "--theme-file" => theme_file = args.next().map(PathBuf::from),
            "--" => locations.extend(args.map(PathBuf::from)),
            _ if arg.starts_with('-') => {
                eprintln!("Unknown argument: {arg}. See --help.");
                return Ok(());
            }
            _ => locations.push(PathBuf::from(arg)),
        }
    }
    let icon = image::load_from_memory(include_bytes!("../assets/rice-explorer.png"))
        .expect("embedded PNG icon")
        .to_rgba8();
    let (width, height) = icon.dimensions();
    let icon = eframe::egui::IconData { rgba: icon.into_raw(), width, height };
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
