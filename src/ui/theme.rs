//! ockr shell theming — Story 12.
//!
//! Themes are defined as TOML files in the `themes/` directory.  Two themes
//! ship bundled: **Oxide** (warm dark) and **Ochre** (warm light).
//!
//! ## Runtime access
//!
//! `ThemePalette` is stored as a GPUI global.  Read it from any render method
//! with `cx.global::<ThemePalette>()`.
//!
//! ## WCAG AA
//!
//! Contrast ratios between each text colour and its expected background are
//! validated at theme-load time.  Themes that fail the 4.5 : 1 ratio log a
//! warning (they are not rejected, so custom themes can be iterated quickly).

use serde::Deserialize;

// ── Parsed theme file ─────────────────────────────────────────────────────────

/// Intermediate deserialisation target matching the TOML schema.
#[derive(Debug, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    pub variant: String, // "dark" | "light"

    pub bg_base: String,
    pub bg_panel: String,
    pub bg_surface: String,
    pub bg_hover: String,

    pub border: String,
    pub border_subtle: String,

    pub ochre: String,
    pub ochre_dim: String,
    pub ochre_border: String,

    pub text: String,
    pub text_muted: String,
    pub text_subtle: String,
    pub text_faint: String,

    pub mode_insert: String,
    pub mode_normal: String,
    pub mode_visual: String,

    pub cursor_fg: String,

    pub syntax_heading: String,
    pub syntax_keyword: String,
    pub syntax_math: String,
    pub syntax_link: String,
    pub syntax_code: String,
    pub syntax_comment: String,
}

// ── Runtime palette ───────────────────────────────────────────────────────────

/// Semantic colour palette used by all shell UI elements.
///
/// All colour values are packed RGB `u32`s (`0xRRGGBB`) ready to pass to
/// `gpui::rgb(value)`.
///
/// Stored as a GPUI global — access via `cx.global::<ThemePalette>()`.
#[derive(Debug, Clone)]
pub struct ThemePalette {
    pub name: String,

    pub bg_base: u32,
    pub bg_panel: u32,
    pub bg_surface: u32,
    pub bg_hover: u32,

    pub border: u32,
    pub border_subtle: u32,

    pub ochre: u32,
    pub ochre_dim: u32,
    pub ochre_border: u32,

    pub text: u32,
    pub text_muted: u32,
    pub text_subtle: u32,
    pub text_faint: u32,

    pub mode_insert: u32,
    pub mode_normal: u32,
    pub mode_visual: u32,

    pub cursor_fg: u32,

    pub syntax_heading: u32,
    pub syntax_keyword: u32,
    pub syntax_math: u32,
    pub syntax_link: u32,
    pub syntax_code: u32,
    pub syntax_comment: u32,
}

impl gpui::Global for ThemePalette {}

impl ThemePalette {
    /// Parse a TOML string into a validated `ThemePalette`.
    ///
    /// WCAG AA contrast failures are logged as warnings.
    pub fn from_toml(src: &str) -> Result<Self, String> {
        let raw: ThemeFile =
            toml::from_str(src).map_err(|e| format!("theme parse error: {e}"))?;
        let p = Self::from_file(raw);
        p.validate_wcag_aa();
        Ok(p)
    }

    fn from_file(f: ThemeFile) -> Self {
        Self {
            name: f.name,
            bg_base: hex(&f.bg_base),
            bg_panel: hex(&f.bg_panel),
            bg_surface: hex(&f.bg_surface),
            bg_hover: hex(&f.bg_hover),
            border: hex(&f.border),
            border_subtle: hex(&f.border_subtle),
            ochre: hex(&f.ochre),
            ochre_dim: hex(&f.ochre_dim),
            ochre_border: hex(&f.ochre_border),
            text: hex(&f.text),
            text_muted: hex(&f.text_muted),
            text_subtle: hex(&f.text_subtle),
            text_faint: hex(&f.text_faint),
            mode_insert: hex(&f.mode_insert),
            mode_normal: hex(&f.mode_normal),
            mode_visual: hex(&f.mode_visual),
            cursor_fg: hex(&f.cursor_fg),
            syntax_heading: hex(&f.syntax_heading),
            syntax_keyword: hex(&f.syntax_keyword),
            syntax_math: hex(&f.syntax_math),
            syntax_link: hex(&f.syntax_link),
            syntax_code: hex(&f.syntax_code),
            syntax_comment: hex(&f.syntax_comment),
        }
    }

    /// Log a warning for any text/background pair that fails WCAG AA (4.5 : 1).
    fn validate_wcag_aa(&self) {
        let pairs: &[(&str, u32, u32)] = &[
            ("text on bg_panel",   self.text,        self.bg_panel),
            ("text_muted on bg_panel", self.text_muted, self.bg_panel),
            ("text on bg_surface", self.text,        self.bg_surface),
            ("ochre on bg_base",   self.ochre,       self.bg_base),
        ];
        for &(label, fg, bg) in pairs {
            let ratio = contrast_ratio(fg, bg);
            if ratio < 4.5 {
                eprintln!(
                    "[ockr theme] WCAG AA warning: \"{}\" contrast {:.2} < 4.5 (theme: {})",
                    label, ratio, self.name
                );
            }
        }
    }

    /// Return the appropriate bundled theme for the current macOS appearance.
    pub fn for_system_appearance() -> Self {
        if system_is_dark() {
            Self::oxide()
        } else {
            Self::ochre()
        }
    }

    /// The bundled Oxide (dark) theme.
    pub fn oxide() -> Self {
        Self::from_toml(include_str!("../../themes/oxide.toml"))
            .expect("bundled Oxide theme must be valid")
    }

    /// The bundled Ochre (light) theme.
    pub fn ochre() -> Self {
        Self::from_toml(include_str!("../../themes/ochre.toml"))
            .expect("bundled Ochre theme must be valid")
    }
}

// ── System appearance detection ───────────────────────────────────────────────

/// Returns `true` if macOS is currently in Dark Mode.
///
/// Reads the `AppleInterfaceStyle` user default — present and equal to `"Dark"`
/// in dark mode, absent in light mode.  Falls back to `true` (dark) on any
/// error so the app looks correct on most developer machines.
fn system_is_dark() -> bool {
    // `defaults read -g AppleInterfaceStyle` prints "Dark\n" or exits non-zero.
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .eq_ignore_ascii_case("dark")
        })
        .unwrap_or(true) // default dark
}

// ── Colour helpers ────────────────────────────────────────────────────────────

/// Parse a CSS hex colour string (`"#RRGGBB"` or `"RRGGBB"`) into a packed `u32`.
fn hex(s: &str) -> u32 {
    let s = s.trim_start_matches('#');
    u32::from_str_radix(s, 16).unwrap_or_else(|_| {
        eprintln!("[ockr theme] invalid colour value: #{s}");
        0xFF00FF // hot-pink sentinel so bad values are immediately visible
    })
}

/// WCAG 2.1 contrast ratio between two packed-RGB colours.
fn contrast_ratio(fg: u32, bg: u32) -> f64 {
    let l1 = relative_luminance(fg);
    let l2 = relative_luminance(bg);
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

fn relative_luminance(color: u32) -> f64 {
    let r = linearise(((color >> 16) & 0xFF) as f64 / 255.0);
    let g = linearise(((color >> 8) & 0xFF) as f64 / 255.0);
    let b = linearise((color & 0xFF) as f64 / 255.0);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn linearise(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055_f64).powf(2.4)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oxide_parses_without_error() {
        let p = ThemePalette::oxide();
        assert_eq!(p.name, "Oxide");
        assert_eq!(p.bg_base, 0x0A0A0A);
        assert_eq!(p.text, 0xF4F4F5);
    }

    #[test]
    fn ochre_parses_without_error() {
        let p = ThemePalette::ochre();
        assert_eq!(p.name, "Ochre");
        assert!(p.bg_base > 0x808080); // light background
    }

    #[test]
    fn contrast_ratio_black_white() {
        let ratio = contrast_ratio(0xFFFFFF, 0x000000);
        assert!((ratio - 21.0).abs() < 0.1, "black/white should be 21:1, got {ratio}");
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(hex("#CC7722"), 0xCC7722);
        assert_eq!(hex("CC7722"), 0xCC7722);
        assert_eq!(hex("#0A0A0A"), 0x0A0A0A);
    }
}
