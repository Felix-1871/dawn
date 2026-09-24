// SPDX-License-Identifier: GPL-3.0-or-later

//! systemd's `kbd-model-map` (`/usr/share/systemd/kbd-model-map`): the
//! table systemd-localed pairs XKB keyboard layouts with console keymaps
//! by, and firmware language tags with layouts. Dawn reads it both ways:
//! the backend for the installed system's console keymap (step 7), the
//! GUI to pre-select a layout from the chosen locale. Callers pass the
//! file's contents; nothing here reads files.

/// Where systemd installs the table.
pub const KBD_MODEL_MAP: &str = "/usr/share/systemd/kbd-model-map";

/// One row: a console keymap and the XKB settings it corresponds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapRow {
    pub console: String,
    /// One or more XKB layouts, comma-separated, as in `mk,us`.
    pub layout: String,
    /// Empty for `-`.
    pub variant: String,
    /// BCP 47 language tags the row suits, such as `de-DE` or `de`.
    pub languages: Vec<String>,
}

/// The rows of a `kbd-model-map`, skipping comments and malformed lines.
/// The language column is newer than the rest and may be missing.
pub fn parse(map: &str) -> Vec<KeymapRow> {
    map.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 5 {
                return None;
            }
            let dash_empty = |field: &str| {
                if field == "-" {
                    String::new()
                } else {
                    field.to_string()
                }
            };
            Some(KeymapRow {
                console: fields[0].to_string(),
                layout: fields[1].to_string(),
                variant: dash_empty(fields[3]),
                languages: fields
                    .get(5)
                    .filter(|tags| **tags != "-")
                    .map(|tags| tags.split(',').map(str::to_string).collect())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// `s` is `prefix`, or `prefix` followed by a comma and more layouts.
fn starts_with_layouts(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(','))
}

/// How well a row's layouts match ours, as systemd-localed's
/// `find_legacy_keymap` scores it.
fn layout_score(ours: &str, theirs: &str) -> u32 {
    if ours == theirs {
        return 10;
    }
    let reversed: Vec<&str> = theirs.split(',').rev().collect();
    if ours == reversed.join(",") {
        return 9;
    }
    if starts_with_layouts(ours, theirs) {
        return 5;
    }
    let first = theirs.split(',').next().unwrap_or(theirs);
    if starts_with_layouts(ours, first) {
        return 1;
    }
    0
}

/// The console keymap for an XKB layout and variant, chosen the way
/// systemd-localed converts `localectl set-x11-keymap` settings: the best
/// layout match, a matching variant breaking ties, the first row winning
/// among equals. `None` when no row has the layout at all.
pub fn console_keymap(rows: &[KeymapRow], layout: &str, variant: &str) -> Option<String> {
    let mut best: Option<(u32, &str)> = None;
    for row in rows {
        let mut score = layout_score(layout, &row.layout);
        if score == 0 {
            continue;
        }
        // localed adds one for the XKB model, which Dawn never sets and
        // so always matches, then one for the variant.
        score += 1;
        if row.variant == variant {
            score += 1;
        }
        if best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, &row.console));
        }
    }
    best.map(|(_, console)| console.to_string())
}

/// The XKB layout and variant of the first row listing `tag`, for
/// pre-selecting a keyboard from a locale. A row for several layouts
/// (`mk,us`, with a key to switch) contributes only its first.
pub fn layout_for_language(rows: &[KeymapRow], tag: &str) -> Option<(String, String)> {
    let row = rows
        .iter()
        .find(|row| row.languages.iter().any(|language| language == tag))?;
    let first = |value: &str| value.split(',').next().unwrap_or_default().to_string();
    Some((first(&row.layout), first(&row.variant)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rows from systemd 261's kbd-model-map.
    const MAP: &str = "\
# consolelayout\t\txlayout\txmodel\t\txvariant\txoptions\t\t\t\t\tbcp47
sg\t\t\tch\tpc105\t\tde_nodeadkeys\tterminate:ctrl_alt_bksp\t\t\t\tde-CH
mk-utf\t\t\tmk,us\tpc105\t\t-\t\tterminate:ctrl_alt_bksp,grp:shifts_toggle,grp_led:scroll\tmk-MK,mk
uk\t\t\tgb\tpc105\t\t-\t\tterminate:ctrl_alt_bksp\t\t\t\ten-GB
de\t\t\tde\tpc105\t\t-\t\tterminate:ctrl_alt_bksp\t\t\t\tde-DE,de-AT,de
la-latin1\t\tlatam\tpc105\t\t-\t\tterminate:ctrl_alt_bksp\t\t\t\tes-419,es-MX,es-AR
us\t\t\tus\tpc105+inet\t-\t\tterminate:ctrl_alt_bksp\t\t\t\ten-US,en
de-latin1\t\tde\tpc105\t\t-\t\tterminate:ctrl_alt_bksp\t\t\t\t-
de-latin1-nodeadkeys\tde\tpc105\t\tnodeadkeys\tterminate:ctrl_alt_bksp\t\t\t\t-
fr_CH\t\t\tch\tpc105\t\tfr\t\tterminate:ctrl_alt_bksp\t\t\t\tfr-CH
old-format\t\tzz\tpc105\t\t-\t\t-
";

    fn keymap(layout: &str, variant: &str) -> Option<String> {
        console_keymap(&parse(MAP), layout, variant)
    }

    #[test]
    fn parses_rows_and_the_optional_language_column() {
        let rows = parse(MAP);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].console, "sg");
        assert_eq!(rows[0].variant, "de_nodeadkeys");
        assert_eq!(rows[1].languages, ["mk-MK", "mk"]);
        assert_eq!(rows[2].variant, "");
        assert!(rows[9].languages.is_empty());
    }

    #[test]
    fn a_layout_whose_console_name_differs_maps_through_the_table() {
        assert_eq!(keymap("gb", "").as_deref(), Some("uk"));
        assert_eq!(keymap("latam", "").as_deref(), Some("la-latin1"));
    }

    #[test]
    fn the_first_of_equally_good_rows_wins() {
        assert_eq!(keymap("de", "").as_deref(), Some("de"));
    }

    #[test]
    fn a_matching_variant_beats_the_layouts_default() {
        assert_eq!(
            keymap("de", "nodeadkeys").as_deref(),
            Some("de-latin1-nodeadkeys")
        );
        assert_eq!(keymap("ch", "fr").as_deref(), Some("fr_CH"));
    }

    #[test]
    fn an_unknown_variant_falls_back_to_the_layout() {
        assert_eq!(keymap("de", "neo").as_deref(), Some("de"));
    }

    #[test]
    fn a_single_layout_matches_a_row_for_several() {
        assert_eq!(keymap("mk", "").as_deref(), Some("mk-utf"));
    }

    #[test]
    fn a_layout_the_table_lacks_has_no_console_keymap() {
        assert_eq!(keymap("ara", ""), None);
    }

    #[test]
    fn a_language_tag_finds_its_layout() {
        let rows = parse(MAP);
        assert_eq!(
            layout_for_language(&rows, "de-AT"),
            Some(("de".to_string(), String::new()))
        );
        assert_eq!(
            layout_for_language(&rows, "de-CH"),
            Some(("ch".to_string(), "de_nodeadkeys".to_string()))
        );
        assert_eq!(
            layout_for_language(&rows, "mk"),
            Some(("mk".to_string(), String::new()))
        );
        assert_eq!(layout_for_language(&rows, "sv-SE"), None);
    }
}
