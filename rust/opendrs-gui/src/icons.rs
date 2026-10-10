//! The stroke icons of the Python version (`theme._ICONS`, written to `assets/icons` in white by
//! `tools/make_icons.py`), drawn in a colour of the theme as `theme.icon` does. egui_extras
//! rasterises the SVG at the screen's scale, so they stay sharp at any display scaling.

use eframe::egui::{self, Color32, Image, ImageSource, Vec2};

/// The toolbar's icon size (the Python toolbar's 18×18).
pub const TOOLBAR: f32 = 18.0;
/// The problem list's icon size.
pub const LIST: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Check,
    Chip,
    Clean,
    Compare,
    Down,
    Open,
    Plus,
    Radio,
    Read,
    Redo,
    Refresh,
    Save,
    Search,
    Trash,
    Undo,
    Up,
    Users,
    Warning,
    Write,
}

impl Icon {
    pub const ALL: [Icon; 19] = [
        Icon::Check,
        Icon::Chip,
        Icon::Clean,
        Icon::Compare,
        Icon::Down,
        Icon::Open,
        Icon::Plus,
        Icon::Radio,
        Icon::Read,
        Icon::Redo,
        Icon::Refresh,
        Icon::Save,
        Icon::Search,
        Icon::Trash,
        Icon::Undo,
        Icon::Up,
        Icon::Users,
        Icon::Warning,
        Icon::Write,
    ];

    /// The SVG, compiled into the program.
    pub fn source(self) -> ImageSource<'static> {
        match self {
            Icon::Check => egui::include_image!("../assets/icons/check.svg"),
            Icon::Chip => egui::include_image!("../assets/icons/chip.svg"),
            Icon::Clean => egui::include_image!("../assets/icons/clean.svg"),
            Icon::Compare => egui::include_image!("../assets/icons/compare.svg"),
            Icon::Down => egui::include_image!("../assets/icons/down.svg"),
            Icon::Open => egui::include_image!("../assets/icons/open.svg"),
            Icon::Plus => egui::include_image!("../assets/icons/plus.svg"),
            Icon::Radio => egui::include_image!("../assets/icons/radio.svg"),
            Icon::Read => egui::include_image!("../assets/icons/read.svg"),
            Icon::Redo => egui::include_image!("../assets/icons/redo.svg"),
            Icon::Refresh => egui::include_image!("../assets/icons/refresh.svg"),
            Icon::Save => egui::include_image!("../assets/icons/save.svg"),
            Icon::Search => egui::include_image!("../assets/icons/search.svg"),
            Icon::Trash => egui::include_image!("../assets/icons/trash.svg"),
            Icon::Undo => egui::include_image!("../assets/icons/undo.svg"),
            Icon::Up => egui::include_image!("../assets/icons/up.svg"),
            Icon::Users => egui::include_image!("../assets/icons/users.svg"),
            Icon::Warning => egui::include_image!("../assets/icons/warning.svg"),
            Icon::Write => egui::include_image!("../assets/icons/write.svg"),
        }
    }

    /// The icon at `size` points in `colour`.
    pub fn image(self, size: f32, colour: Color32) -> Image<'static> {
        Image::new(self.source())
            .fit_to_exact_size(Vec2::splat(size))
            .tint(colour)
    }
}

/// A flat toolbar button with an icon (and text, unless empty), as the Python toolbar shows it.
pub fn tool_button(ui: &mut egui::Ui, icon: Icon, text: &str, enabled: bool) -> egui::Response {
    let image = icon.image(TOOLBAR, ui.visuals().text_color());
    let button = if text.is_empty() {
        egui::Button::image(image)
    } else {
        egui::Button::image_and_text(image, text)
    };
    ui.add_enabled(enabled, button.frame_when_inactive(false))
}

/// The colours `theme.py` gives the problem list's marks: (danger, ok).
pub fn status_colours(dark_mode: bool) -> (Color32, Color32) {
    if dark_mode {
        (
            Color32::from_rgb(0xef, 0x6b, 0x6b),
            Color32::from_rgb(0x66, 0xbb, 0x6a),
        )
    } else {
        (
            Color32::from_rgb(0xc6, 0x28, 0x28),
            Color32::from_rgb(0x2e, 0x7d, 0x32),
        )
    }
}

/// Let `ctx` draw SVG images (once per context).
pub fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
}

/// Rasterise every icon in `ctx` (the self-test's proof that SVG drawing is built in).
pub fn check(ctx: &egui::Context) -> Result<usize, String> {
    install(ctx);
    for icon in Icon::ALL {
        let ImageSource::Bytes { uri, bytes } = icon.source() else {
            return Err(format!("{icon:?} 不是內嵌的圖示"));
        };
        ctx.include_bytes(uri.clone(), bytes);
        let size = egui::load::SizeHint::Size {
            width: 36,
            height: 36,
            maintain_aspect_ratio: true,
        };
        match ctx.try_load_image(&uri, size) {
            Ok(egui::load::ImagePoll::Ready { image }) if image.size == [36, 36] => {}
            Ok(egui::load::ImagePoll::Ready { image }) => {
                return Err(format!("{icon:?} 畫成 {:?}，應為 36×36", image.size));
            }
            Ok(egui::load::ImagePoll::Pending { .. }) => {
                return Err(format!("{icon:?} 沒有畫出來"));
            }
            Err(e) => return Err(format!("{icon:?}：{e}")),
        }
    }
    Ok(Icon::ALL.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterises() {
        assert_eq!(check(&egui::Context::default()), Ok(19));
    }

    #[test]
    fn every_svg_in_the_assets_is_an_icon() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .expect("icons folder")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        let mut names: Vec<String> = Icon::ALL
            .iter()
            .map(|i| format!("{}.svg", format!("{i:?}").to_lowercase()))
            .collect();
        names.sort();
        assert_eq!(files, names);
    }
}
