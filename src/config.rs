use std::collections::HashMap;

use include_dir::{include_dir, Dir};

static TEMPLATES_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/old.python/termtosvg/data/templates");

pub const DEFAULT_TEMPLATES_NAMES: &[&str] = &[
    "base16_default_dark.svg",
    "dracula.svg",
    "gjm8_play.svg",
    "gjm8_single_loop.svg",
    "gjm8.svg",
    "powershell.svg",
    "progress_bar.svg",
    "putty.svg",
    "solarized_dark.svg",
    "solarized_light.svg",
    "terminal_app.svg",
    "ubuntu.svg",
    "window_frame_js.svg",
    "window_frame_powershell.svg",
    "window_frame.svg",
    "xterm.svg",
];

pub fn validate_geometry(screen_geometry: &str) -> Result<(u16, u16), String> {
    let geometry = screen_geometry.to_lowercase();
    let parts: Vec<&str> = geometry.split('x').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid value for screen-geometry option: \"{}\"", screen_geometry));
    }

    let columns: u16 = parts[0]
        .parse()
        .map_err(|_| format!("Invalid value for screen-geometry option: \"{}\"", screen_geometry))?;
    let rows: u16 = parts[1]
        .parse()
        .map_err(|_| format!("Invalid value for screen-geometry option: \"{}\"", screen_geometry))?;

    if columns == 0 || rows == 0 {
        return Err(format!("Invalid value for screen-geometry option: \"{}\"", screen_geometry));
    }

    Ok((columns, rows))
}

pub fn default_templates() -> HashMap<String, Vec<u8>> {
    let mut templates = HashMap::new();
    for name in DEFAULT_TEMPLATES_NAMES {
        if let Some(file) = TEMPLATES_DIR.get_file(name) {
            let mut key = (*name).to_string();
            if let Some(stripped) = key.strip_suffix(".svg") {
                key = stripped.to_string();
            }
            templates.insert(key, file.contents().to_vec());
        }
    }
    templates
}
