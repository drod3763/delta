//! Delta doesn't have a formal concept of a "theme". What it has is
//!
//! 1. The choice of "theme". This is the language syntax highlighting theme; you have to make this
//!    choice when using `bat` also.
//! 2. The choice of "light vs dark mode". This determines whether the background colors should be
//!    chosen for a light or dark terminal background. (`bat` has no equivalent.)
//!
//! Basically:
//! 1. The theme is specified by the `--syntax-theme` option. If this isn't supplied then it is specified
//!    by the `BAT_THEME` environment variable.
//! 2. Light vs dark mode is specified by the `--light` or `--dark` options. If these aren't
//!    supplied then it detected from the terminal. If this fails it is inferred from the chosen theme.
//!
//! In the absence of other factors, the default assumes a dark terminal background.
//!
//! Light vs dark mode is resolved early (before features are gathered) by
//! [`resolve_color_mode_for_feature_injection`], so that the `dark-features` / `light-features`
//! settings can activate different features per mode. The terminal-detection result is cached
//! in `opt.computed.detected_color_mode` and reused here, so the terminal is queried at most
//! once per invocation.

use std::io::{stdout, IsTerminal};

use bat;
use bat::assets::HighlightingAssets;
#[cfg(not(test))]
use terminal_colorsaurus::{color_scheme, QueryOptions};

use crate::cli::{self, DetectDarkLight};
use crate::color::{ColorMode, ColorMode::*};
use crate::git_config::GitConfig;

#[allow(non_snake_case)]
pub fn set__color_mode__syntax_theme__syntax_set(opt: &mut cli::Opt, assets: HighlightingAssets) {
    let (color_mode, syntax_theme_name) =
        get_color_mode_and_syntax_theme_name(opt.syntax_theme.as_ref(), get_color_mode(opt));
    opt.computed.color_mode = color_mode;

    opt.computed.syntax_theme = if is_no_syntax_highlighting_syntax_theme_name(&syntax_theme_name) {
        None
    } else {
        Some(assets.get_theme(&syntax_theme_name).clone())
    };
    opt.computed.syntax_set = assets.get_syntax_set().unwrap().clone();
}

pub fn is_light_syntax_theme(theme: &str) -> bool {
    LIGHT_SYNTAX_THEMES.contains(&theme) || theme.to_lowercase().contains("light")
}

pub fn color_mode_from_syntax_theme(theme: &str) -> ColorMode {
    if is_light_syntax_theme(theme) {
        ColorMode::Light
    } else {
        ColorMode::Dark
    }
}

const LIGHT_SYNTAX_THEMES: [&str; 7] = [
    "Catppuccin Latte",
    "GitHub",
    "gruvbox-light",
    "gruvbox-white",
    "Monokai Extended Light",
    "OneHalfLight",
    "Solarized (light)",
];

const DEFAULT_LIGHT_SYNTAX_THEME: &str = "GitHub";
const DEFAULT_DARK_SYNTAX_THEME: &str = "Monokai Extended";

fn is_no_syntax_highlighting_syntax_theme_name(theme_name: &str) -> bool {
    theme_name.to_lowercase() == "none"
}

/// Return a (theme_name, color_mode) tuple.
/// theme_name == None in return value means syntax highlighting is disabled.
fn get_color_mode_and_syntax_theme_name(
    syntax_theme: Option<&String>,
    mode: Option<ColorMode>,
) -> (ColorMode, String) {
    match (syntax_theme, mode) {
        (Some(theme), None) => (color_mode_from_syntax_theme(theme), theme.to_string()),
        (Some(theme), Some(mode)) => (mode, theme.to_string()),
        (None, None | Some(Dark)) => (Dark, DEFAULT_DARK_SYNTAX_THEME.to_string()),
        (None, Some(Light)) => (Light, DEFAULT_LIGHT_SYNTAX_THEME.to_string()),
    }
}

fn get_color_mode(opt: &cli::Opt) -> Option<ColorMode> {
    if opt.light {
        Some(Light)
    } else if opt.dark {
        Some(Dark)
    } else {
        // Reuse the result of any earlier terminal detection (performed by
        // `resolve_color_mode_for_feature_injection` before feature gathering) rather than
        // querying the terminal a second time.
        opt.computed.detected_color_mode
    }
}

/// Resolve the effective color mode *before* features are gathered, so that per-mode feature
/// lists (`dark-features` / `light-features`) can be activated according to the detected mode.
///
/// The trigger deliberately considers only sources that are independent of the feature list,
/// to avoid a circular dependency (an activated theme typically sets `dark = true` itself).
/// It mirrors the precedence later applied by [`get_color_mode`] +
/// [`get_color_mode_and_syntax_theme_name`]:
///   1. the `--light` / `--dark` command-line flags,
///   2. the `light` / `dark` keys in the main `[delta]` git config section (read directly,
///      not via the feature machinery),
///   3. terminal detection (subject to `--detect-dark-light`),
///   4. the syntax theme, when none of the above resolve a mode — a light syntax theme
///      implies light mode and vice versa, just as the final resolution does,
///   5. otherwise the default dark mode.
///
/// Steps 4 and 5 mirror `get_color_mode_and_syntax_theme_name`'s handling of a `None` mode, so
/// the mode chosen here always matches the mode the renderer ends up in. The function therefore
/// never returns `None`; it returns `Option` only to compose with the call site.
///
/// The terminal-detection result is cached in `opt.computed.detected_color_mode` so that the
/// later call to `get_color_mode` reuses it instead of querying the terminal again.
pub fn resolve_color_mode_for_feature_injection(
    opt: &mut cli::Opt,
    git_config: &mut Option<GitConfig>,
) -> Option<ColorMode> {
    if opt.light {
        return Some(Light);
    }
    if opt.dark {
        return Some(Dark);
    }
    // The syntax theme set on the command line or via BAT_THEME is already on `opt`; the
    // main-section `delta.syntax-theme` is resolved later, so read it directly here.
    let mut syntax_theme = opt.syntax_theme.clone();
    if let Some(git_config) = git_config {
        // Read the main-section keys directly; do not consult enabled features (that would be
        // circular). If both are literally set, prefer Dark here and let the existing
        // `validate_light_and_dark` raise the error later.
        let main_dark = git_config.get::<bool>("delta.dark").unwrap_or(false);
        let main_light = git_config.get::<bool>("delta.light").unwrap_or(false);
        if main_dark {
            return Some(Dark);
        }
        if main_light {
            return Some(Light);
        }
        if syntax_theme.is_none() {
            syntax_theme = git_config.get::<String>("delta.syntax-theme");
        }
    }
    if should_detect_color_mode(opt) {
        let detected = detect_color_mode();
        opt.computed.detected_color_mode = detected;
        if detected.is_some() {
            return detected;
        }
    }
    // No explicit mode and no detection result: infer the mode from the syntax theme, and
    // otherwise fall back to dark — exactly the `(Some(theme), None)` / `(None, None)` branches
    // of `get_color_mode_and_syntax_theme_name`, so per-mode features match the rendered mode.
    Some(
        syntax_theme
            .as_deref()
            .filter(|theme| !is_no_syntax_highlighting_syntax_theme_name(theme))
            .map(color_mode_from_syntax_theme)
            .unwrap_or(Dark),
    )
}

/// See [`cli::Opt::detect_dark_light`] for a detailed explanation.
fn should_detect_color_mode(opt: &cli::Opt) -> bool {
    match opt.detect_dark_light {
        DetectDarkLight::Auto => opt.color_only || stdout().is_terminal(),
        DetectDarkLight::Always => true,
        DetectDarkLight::Never => false,
    }
}

#[cfg(not(test))]
fn detect_color_mode() -> Option<ColorMode> {
    color_scheme(QueryOptions::default())
        .ok()
        .map(ColorMode::from)
}

impl From<terminal_colorsaurus::ColorScheme> for ColorMode {
    fn from(value: terminal_colorsaurus::ColorScheme) -> Self {
        match value {
            terminal_colorsaurus::ColorScheme::Dark => ColorMode::Dark,
            terminal_colorsaurus::ColorScheme::Light => ColorMode::Light,
        }
    }
}

#[cfg(test)]
fn detect_color_mode() -> Option<ColorMode> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color;
    use crate::tests::integration_test_utils;

    // TODO: Test influence of BAT_THEME env var. E.g. see utils::process::tests::FakeParentArgs.
    #[test]
    fn test_syntax_theme_selection() {
        for (
            syntax_theme,
            mode, // (--light, --dark)
            expected_syntax_theme,
            expected_mode,
        ) in vec![
            (None, None, DEFAULT_DARK_SYNTAX_THEME, Dark),
            (Some("GitHub"), None, "GitHub", Light),
            (Some("Nord"), None, "Nord", Dark),
            (None, Some(Dark), DEFAULT_DARK_SYNTAX_THEME, Dark),
            (None, Some(Light), DEFAULT_LIGHT_SYNTAX_THEME, Light),
            (Some("GitHub"), Some(Light), "GitHub", Light),
            (Some("GitHub"), Some(Dark), "GitHub", Dark),
            (Some("Nord"), Some(Light), "Nord", Light),
            (Some("Nord"), Some(Dark), "Nord", Dark),
            (Some("none"), None, "none", Dark),
            (Some("none"), Some(Dark), "none", Dark),
            (Some("None"), Some(Light), "none", Light),
        ] {
            let mut args = vec![];
            if let Some(syntax_theme) = syntax_theme {
                args.push("--syntax-theme");
                args.push(syntax_theme);
            }
            let is_true_color = true;
            if is_true_color {
                args.push("--true-color");
                args.push("always");
            } else {
                args.push("--true-color");
                args.push("never");
            }
            match mode {
                Some(Light) => {
                    args.push("--light");
                }
                Some(Dark) => {
                    args.push("--dark");
                }
                None => {}
            }
            let config = integration_test_utils::make_config_from_args(&args);
            assert_eq!(
                &config
                    .syntax_theme
                    .clone()
                    .map(|t| t.name.unwrap())
                    .unwrap_or("none".to_string()),
                expected_syntax_theme
            );
            if is_no_syntax_highlighting_syntax_theme_name(expected_syntax_theme) {
                assert!(config.syntax_theme.is_none())
            } else {
                assert_eq!(
                    config.syntax_theme.unwrap().name.as_ref().unwrap(),
                    expected_syntax_theme
                );
            }
            assert_eq!(
                config.minus_style.ansi_term_style.background.unwrap(),
                color::get_minus_background_color_default(expected_mode, is_true_color)
            );
            assert_eq!(
                config.minus_emph_style.ansi_term_style.background.unwrap(),
                color::get_minus_emph_background_color_default(expected_mode, is_true_color)
            );
            assert_eq!(
                config.plus_style.ansi_term_style.background.unwrap(),
                color::get_plus_background_color_default(expected_mode, is_true_color)
            );
            assert_eq!(
                config.plus_emph_style.ansi_term_style.background.unwrap(),
                color::get_plus_emph_background_color_default(expected_mode, is_true_color)
            );
        }
    }
}
