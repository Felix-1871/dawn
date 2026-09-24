// SPDX-License-Identifier: GPL-3.0-or-later

//! Loads a branding folder into the Slint `Theme` global at startup
//! (SPEC.md "Branding & configuration"). Never baked in at compile
//! time, so swapping the folder never needs a rebuild.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BrandingToml {
    product_name: String,
    version_string: String,
    accent_color: String,
    website: String,
    support_url: String,
}

pub struct Branding {
    pub product_name: String,
    pub version_string: String,
    pub accent_color: slint::Color,
    pub website: String,
    pub support_url: String,
    pub logo: slint::Image,
    pub icon: slint::Image,
    pub welcome_text: String,
}

/// `/usr/share/dawn/branding/<name>/` is where a real install looks
/// (SPEC.md). `DAWN_BRANDING_DIR` overrides it, and a path relative to
/// this crate is the fallback for running Dawn straight out of the
/// workspace during development.
pub fn find_branding_dir(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("DAWN_BRANDING_DIR") {
        return PathBuf::from(dir);
    }
    let system_path = PathBuf::from(format!("/usr/share/dawn/branding/{name}"));
    if system_path.is_dir() {
        return system_path;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../branding")
        .join(name)
}

pub fn load(dir: &Path) -> Result<Branding, String> {
    let toml_path = dir.join("branding.toml");
    let raw = std::fs::read_to_string(&toml_path)
        .map_err(|err| format!("could not read {}: {err}", toml_path.display()))?;
    let parsed: BrandingToml = toml::from_str(&raw)
        .map_err(|err| format!("could not parse {}: {err}", toml_path.display()))?;

    let accent_color = parse_hex_color(&parsed.accent_color).ok_or_else(|| {
        format!(
            "invalid accent_color {:?} in {}",
            parsed.accent_color,
            toml_path.display()
        )
    })?;

    let logo = load_image(&dir.join("logo.svg"))?;
    let icon = load_image(&dir.join("icon.svg"))?;

    let welcome_text = std::fs::read_to_string(dir.join("welcome.md"))
        .unwrap_or_default()
        .trim()
        .to_string();

    Ok(Branding {
        product_name: parsed.product_name,
        version_string: parsed.version_string,
        accent_color,
        website: parsed.website,
        support_url: parsed.support_url,
        logo,
        icon,
        welcome_text,
    })
}

fn load_image(path: &Path) -> Result<slint::Image, String> {
    slint::Image::load_from_path(path)
        .map_err(|err| format!("could not load {}: {err}", path.display()))
}

fn parse_hex_color(value: &str) -> Option<slint::Color> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(slint::Color::from_rgb_u8(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_hex_color() {
        let color = parse_hex_color("#5b8dee").unwrap();
        assert_eq!(
            (color.red(), color.green(), color.blue()),
            (0x5b, 0x8d, 0xee)
        );
    }

    #[test]
    fn rejects_a_malformed_hex_color() {
        assert!(parse_hex_color("5b8dee").is_none());
        assert!(parse_hex_color("#5b8d").is_none());
    }
}
