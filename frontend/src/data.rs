// SPDX-License-Identifier: GPL-3.0-or-later

//! The lists the Welcome, Keyboard and Timezone screens choose from, read
//! from the live system's own data (SPEC.md M4, "keyboard and timezone
//! data"): glibc's supported locales and their names, xkeyboard-config's
//! layouts and variants, tzdata's zones and country names, and systemd's
//! kbd-model-map for guessing a keyboard from the locale. SPEC.md's
//! Timezone screen is "guessed from the chosen locale"; the keyboard is
//! too (DECISIONS.md, M4).
//!
//! Loading never fails: whatever can't be read leaves a one-entry list
//! (`en_US.UTF-8`, `us`, `UTC`), so Dawn still installs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use plan::keymap::{self, KeymapRow};

/// Where the lists are read from.
#[derive(Clone, Debug)]
pub struct DataFiles {
    /// glibc's `SUPPORTED`: every locale `locale-gen` can build.
    pub supported_locales: PathBuf,
    /// glibc's locale sources, for each locale's language and territory
    /// names.
    pub locale_sources: PathBuf,
    /// xkeyboard-config's list of layouts and variants.
    pub xkb_rules: PathBuf,
    /// systemd's kbd-model-map.
    pub kbd_model_map: PathBuf,
    /// tzdata's zones, with the country each is in, most populous first.
    pub zone_tab: PathBuf,
    /// tzdata's country names.
    pub iso3166_tab: PathBuf,
}

impl DataFiles {
    /// Where an Arch-based system keeps them.
    pub fn system() -> Self {
        Self {
            supported_locales: PathBuf::from("/usr/share/i18n/SUPPORTED"),
            locale_sources: PathBuf::from("/usr/share/i18n/locales"),
            xkb_rules: PathBuf::from("/usr/share/X11/xkb/rules/evdev.lst"),
            kbd_model_map: PathBuf::from(keymap::KBD_MODEL_MAP),
            zone_tab: PathBuf::from("/usr/share/zoneinfo/zone.tab"),
            iso3166_tab: PathBuf::from("/usr/share/zoneinfo/iso3166.tab"),
        }
    }
}

/// One entry of a list: what goes into the plan, what the list shows,
/// and a second, dimmer column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub detail: String,
}

/// Everything the three screens choose from.
#[derive(Debug, Default)]
pub struct Data {
    /// Values are locale names as `SUPPORTED` and `locale.gen` spell
    /// them (`de_DE.UTF-8`, `aa_ER`).
    pub locales: Vec<Choice>,
    /// Values are XKB's `layout` or `layout(variant)`; see
    /// [`split_keyboard`].
    pub keyboards: Vec<Choice>,
    /// Values are zone names (`Europe/Berlin`).
    pub timezones: Vec<Choice>,
    first_zone: HashMap<String, String>,
    keymap_rows: Vec<KeymapRow>,
}

pub const UTC: &str = "Etc/UTC";

/// Countries whose first zone in `zone.tab` isn't where most people
/// live: tzdata lists a country's zones by geography, not population
/// (Brazil starts with its Atlantic islands, Russia with Kaliningrad).
const MAIN_ZONES: &[(&str, &str)] = &[
    ("AU", "Australia/Sydney"),
    ("BR", "America/Sao_Paulo"),
    ("CA", "America/Toronto"),
    ("RU", "Europe/Moscow"),
    ("UA", "Europe/Kyiv"),
    ("UZ", "Asia/Tashkent"),
];

/// `de(nodeadkeys)` for a variant, `de` for the layout's default.
pub fn keyboard_value(layout: &str, variant: &str) -> String {
    if variant.is_empty() {
        layout.to_string()
    } else {
        format!("{layout}({variant})")
    }
}

/// The layout and variant in a keyboard [`Choice`]'s value.
pub fn split_keyboard(value: &str) -> (String, String) {
    match value.split_once('(') {
        Some((layout, variant)) => (
            layout.to_string(),
            variant.trim_end_matches(')').to_string(),
        ),
        None => (value.to_string(), String::new()),
    }
}

impl Data {
    pub fn load(files: &DataFiles) -> Self {
        let read = |path: &Path| std::fs::read_to_string(path).ok();

        let mut locales = read(&files.supported_locales)
            .map(|supported| {
                parse_supported(&supported)
                    .into_iter()
                    .map(|name| {
                        let source = files.locale_sources.join(name.replace(".UTF-8", ""));
                        let label = std::fs::read(&source)
                            .ok()
                            .and_then(|bytes| locale_label(&name, &String::from_utf8_lossy(&bytes)))
                            .unwrap_or_else(|| name.clone());
                        Choice {
                            detail: name.clone(),
                            value: name,
                            label,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if locales.is_empty() {
            locales.push(Choice {
                value: "en_US.UTF-8".to_string(),
                label: "American English (United States)".to_string(),
                detail: "en_US.UTF-8".to_string(),
            });
        }

        let mut keyboards = read(&files.xkb_rules)
            .map(|rules| parse_xkb_rules(&rules))
            .unwrap_or_default();
        if keyboards.is_empty() {
            keyboards.push(Choice {
                value: "us".to_string(),
                label: "English (US)".to_string(),
                detail: "us".to_string(),
            });
        }

        let countries = read(&files.iso3166_tab)
            .map(|tab| parse_iso3166(&tab))
            .unwrap_or_default();
        let zones = read(&files.zone_tab)
            .map(|tab| parse_zone_tab(&tab))
            .unwrap_or_default();
        let mut first_zone = HashMap::new();
        let mut timezones: Vec<Choice> = zones
            .iter()
            .map(|(country, zone)| {
                first_zone
                    .entry(country.clone())
                    .or_insert_with(|| zone.clone());
                Choice {
                    value: zone.clone(),
                    label: zone.replace('_', " "),
                    detail: countries.get(country).cloned().unwrap_or_default(),
                }
            })
            .collect();
        for (country, zone) in MAIN_ZONES {
            if timezones.iter().any(|choice| choice.value == *zone) {
                first_zone.insert((*country).to_string(), (*zone).to_string());
            }
        }
        timezones.push(Choice {
            value: UTC.to_string(),
            label: "UTC".to_string(),
            detail: "Coordinated Universal Time".to_string(),
        });

        for list in [&mut locales, &mut keyboards, &mut timezones] {
            list.sort_by_cached_key(|choice| choice.label.to_lowercase());
            disambiguate(list);
        }

        let keymap_rows = read(&files.kbd_model_map)
            .map(|map| keymap::parse(&map))
            .unwrap_or_default();

        Self {
            locales,
            keyboards,
            timezones,
            first_zone,
            keymap_rows,
        }
    }

    /// The main zone of the locale's country: the first `zone.tab` lists,
    /// or [`MAIN_ZONES`]'s.
    pub fn timezone_for_locale(&self, locale: &str) -> Option<String> {
        let (_, territory) = language_and_territory(locale);
        self.first_zone.get(territory?).cloned()
    }

    /// The keyboard (as a [`Choice`] value) kbd-model-map pairs with the
    /// locale's language and country, else a layout named after the
    /// country, as many are.
    pub fn keyboard_for_locale(&self, locale: &str) -> Option<String> {
        let (language, territory) = language_and_territory(locale);
        let offered = |value: &str| self.keyboards.iter().any(|choice| choice.value == value);
        let tags = territory
            .map(|territory| format!("{language}-{territory}"))
            .into_iter()
            .chain([language.to_string()]);
        for tag in tags {
            if let Some((layout, variant)) = keymap::layout_for_language(&self.keymap_rows, &tag) {
                let value = keyboard_value(&layout, &variant);
                if offered(&value) {
                    return Some(value);
                }
            }
        }
        let layout = territory?.to_lowercase();
        offered(&layout).then_some(layout)
    }

    pub fn label<'a>(list: &'a [Choice], value: &str) -> Option<&'a str> {
        list.iter()
            .find(|choice| choice.value == value)
            .map(|choice| choice.label.as_str())
    }
}

/// The entries of `list` matching `query`, case-insensitively, in the
/// label, the detail column or the value, with `_` and spaces alike
/// (`new york` finds `America/New_York`). An exact match comes first,
/// then labels starting with the query, then the rest, each in the
/// list's order: typing `us` puts English (US) at the top, not below
/// every Belarusian and Russian layout.
pub fn search(list: &[Choice], query: &str) -> Vec<Choice> {
    let normalise = |text: &str| text.to_lowercase().replace('_', " ");
    let query = normalise(query.trim());
    let mut found: Vec<(u8, Choice)> = list
        .iter()
        .filter_map(|choice| {
            let label = normalise(&choice.label);
            let value = normalise(&choice.value);
            let rank = if query.is_empty() || label == query || value == query {
                0
            } else if label.starts_with(&query) {
                1
            } else if label.contains(&query)
                || value.contains(&query)
                || normalise(&choice.detail).contains(&query)
            {
                2
            } else {
                return None;
            };
            Some((rank, choice.clone()))
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, choice)| choice).collect()
}

/// Two entries with the same label get their values added, so each can
/// still be told apart, and picked by name.
fn disambiguate(list: &mut [Choice]) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for choice in list.iter() {
        *seen.entry(choice.label.clone()).or_default() += 1;
    }
    for choice in list.iter_mut() {
        if seen.get(&choice.label).is_some_and(|count| *count > 1) {
            choice.label = format!("{} — {}", choice.label, choice.value);
        }
    }
}

/// `de_DE.UTF-8` is German in Germany, `ca_ES@valencia` Catalan in Spain,
/// `eo` Esperanto nowhere in particular.
fn language_and_territory(locale: &str) -> (&str, Option<&str>) {
    let base = locale.split(['.', '@']).next().unwrap_or(locale);
    match base.split_once('_') {
        Some((language, territory)) => (language, Some(territory)),
        None => (base, None),
    }
}

/// The UTF-8 locales in glibc's `SUPPORTED`, by the name `locale.gen`
/// uses. `C.UTF-8` is left out: it's no one's language.
fn parse_supported(supported: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    supported
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (name, charset) = line.split_once(char::is_whitespace)?;
            (charset.trim() == "UTF-8" && name != "C.UTF-8" && seen.insert(name.to_string()))
                .then(|| name.to_string())
        })
        .collect()
}

/// "German (Germany)" from a locale source's `language` and `territory`,
/// with any modifier: "Serbian (Serbia, latin)".
fn locale_label(name: &str, source: &str) -> Option<String> {
    let field = |key: &str| {
        source.lines().find_map(|line| {
            let rest = line.trim().strip_prefix(key)?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            let value = rest.trim();
            Some(value.strip_prefix('"')?.strip_suffix('"')?.to_string())
        })
    };
    let language = field("language")?;
    let modifier = name.split_once('@').map(|(_, modifier)| modifier);
    Some(match (field("territory"), modifier) {
        (Some(territory), Some(modifier)) => format!("{language} ({territory}, {modifier})"),
        (Some(territory), None) => format!("{language} ({territory})"),
        (None, Some(modifier)) => format!("{language} ({modifier})"),
        (None, None) => language,
    })
}

/// Every layout, and every variant of one, in xkeyboard-config's
/// `evdev.lst`: `! layout` lines are `name  description`, `! variant`
/// lines `name  layout: description`.
fn parse_xkb_rules(rules: &str) -> Vec<Choice> {
    let mut section = "";
    let mut choices = Vec::new();
    for line in rules.lines() {
        if let Some(name) = line.strip_prefix('!') {
            section = name.trim();
            continue;
        }
        let Some((name, description)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let description = description.trim();
        match section {
            "layout" => choices.push(Choice {
                value: name.to_string(),
                label: description.to_string(),
                detail: name.to_string(),
            }),
            "variant" => {
                let Some((layout, description)) = description.split_once(':') else {
                    continue;
                };
                let value = keyboard_value(layout.trim(), name);
                choices.push(Choice {
                    detail: value.clone(),
                    value,
                    label: description.trim().to_string(),
                });
            }
            _ => {}
        }
    }
    choices
}

/// `(country, zone)` pairs in `zone.tab`'s order, which lists each
/// country's most populous zone first.
fn parse_zone_tab(tab: &str) -> Vec<(String, String)> {
    tab.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let country = fields.next()?;
            let _coordinates = fields.next()?;
            let zone = fields.next()?;
            Some((country.to_string(), zone.to_string()))
        })
        .collect()
}

fn parse_iso3166(tab: &str) -> HashMap<String, String> {
    tab.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let (code, name) = line.split_once('\t')?;
            Some((code.to_string(), name.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED: &str = "\
aa_ER UTF-8
aa_ER@saaho UTF-8
C.UTF-8 UTF-8
de_DE.UTF-8 UTF-8
de_DE ISO-8859-1
de_DE@euro ISO-8859-15
en_US.UTF-8 UTF-8
eo UTF-8
";

    const EVDEV_LST: &str = "\
! model
  pc105           Generic 105-key PC
! layout
  us              English (US)
  de              German
  gb              English (UK)
! variant
  nodeadkeys      de: German (no dead keys)
  intl            us: English (US, intl., with dead keys)
! option
  grp             Switching to another layout
";

    const ZONE_TAB: &str = "\
# tzdb timezone descriptions
DE\t+5230+01322\tEurope/Berlin\tmost of Germany
DE\t+4742+00841\tEurope/Busingen\tBusingen
US\t+404251-0740023\tAmerica/New_York\tEastern (most areas)
US\t+421953-0830245\tAmerica/Detroit\tEastern - MI (most areas)
BR\t-0351-03225\tAmerica/Noronha\tAtlantic islands
BR\t-2332-04637\tAmerica/Sao_Paulo\tBrazil (southeast)
";

    #[test]
    fn keeps_utf8_locales_by_their_locale_gen_names() {
        assert_eq!(
            parse_supported(SUPPORTED),
            ["aa_ER", "aa_ER@saaho", "de_DE.UTF-8", "en_US.UTF-8", "eo"]
        );
    }

    #[test]
    fn names_a_locale_by_language_territory_and_modifier() {
        let source = "LC_IDENTIFICATION\ntitle      \"German locale for Germany\"\nlanguage   \"German\"\nterritory  \"Germany\"\nEND LC_IDENTIFICATION\n";
        assert_eq!(
            locale_label("de_DE.UTF-8", source).as_deref(),
            Some("German (Germany)")
        );
        let serbian = "language\t\"Serbian\"\nterritory\t\"Serbia\"\n";
        assert_eq!(
            locale_label("sr_RS@latin", serbian).as_deref(),
            Some("Serbian (Serbia, latin)")
        );
        assert_eq!(locale_label("xx", "title \"x\"\n"), None);
    }

    #[test]
    fn lists_layouts_and_their_variants() {
        let choices = parse_xkb_rules(EVDEV_LST);
        let values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, ["us", "de", "gb", "de(nodeadkeys)", "us(intl)"]);
        assert_eq!(choices[3].label, "German (no dead keys)");
        assert_eq!(
            split_keyboard("de(nodeadkeys)"),
            ("de".into(), "nodeadkeys".into())
        );
        assert_eq!(split_keyboard("de"), ("de".into(), String::new()));
    }

    const KBD_MODEL_MAP: &str = "\
# consolelayout\txlayout\txmodel\txvariant\txoptions\tbcp47
de\tde\tpc105\t-\tterminate:ctrl_alt_bksp\tde-DE,de-AT,de
us\tus\tpc105+inet\t-\tterminate:ctrl_alt_bksp\ten-US,en
";

    /// `Data::load` over these files, written to a fresh directory; the
    /// rest are missing.
    fn load(files: &[(&str, &str)]) -> Data {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("dawn-data-test-{}-{n}", std::process::id()));
        for (name, content) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let data = Data::load(&DataFiles {
            supported_locales: dir.join("SUPPORTED"),
            locale_sources: dir.join("locales"),
            xkb_rules: dir.join("evdev.lst"),
            kbd_model_map: dir.join("kbd-model-map"),
            zone_tab: dir.join("zone.tab"),
            iso3166_tab: dir.join("iso3166.tab"),
        });
        std::fs::remove_dir_all(&dir).ok();
        data
    }

    fn values(list: &[Choice]) -> Vec<&str> {
        list.iter().map(|choice| choice.value.as_str()).collect()
    }

    #[test]
    fn loads_the_lists_and_guesses_from_the_locale() {
        let data = load(&[
            ("SUPPORTED", SUPPORTED),
            (
                "locales/de_DE",
                "language \"German\"\nterritory \"Germany\"\n",
            ),
            (
                "locales/en_US",
                "language \"English\"\nterritory \"United States\"\n",
            ),
            ("evdev.lst", EVDEV_LST),
            ("kbd-model-map", KBD_MODEL_MAP),
            ("zone.tab", ZONE_TAB),
            ("iso3166.tab", "DE\tGermany\nUS\tUnited States\n"),
        ]);

        // Sorted by label; a locale without a source keeps its name.
        let labels: Vec<&str> = data.locales.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "aa_ER",
                "aa_ER@saaho",
                "English (United States)",
                "eo",
                "German (Germany)"
            ]
        );
        assert_eq!(values(&data.keyboards).len(), 5);
        assert_eq!(
            values(&data.timezones),
            [
                "America/Detroit",
                "America/New_York",
                "America/Noronha",
                "America/Sao_Paulo",
                "Europe/Berlin",
                "Europe/Busingen",
                UTC
            ]
        );
        assert_eq!(data.timezones[1].label, "America/New York");
        assert_eq!(data.timezones[1].detail, "United States");
        // Brazil's first zone is its Atlantic islands.
        assert_eq!(
            data.timezone_for_locale("pt_BR.UTF-8").as_deref(),
            Some("America/Sao_Paulo")
        );

        assert_eq!(
            data.timezone_for_locale("de_DE.UTF-8").as_deref(),
            Some("Europe/Berlin")
        );
        assert_eq!(
            data.timezone_for_locale("en_US.UTF-8").as_deref(),
            Some("America/New_York")
        );
        assert_eq!(data.timezone_for_locale("eo"), None);

        assert_eq!(
            data.keyboard_for_locale("de_DE.UTF-8").as_deref(),
            Some("de")
        );
        // No row for en-GB, but the language's row is for us.
        assert_eq!(
            data.keyboard_for_locale("en_GB.UTF-8").as_deref(),
            Some("us")
        );
        // Nothing in the table: a layout named after the country.
        assert_eq!(data.keyboard_for_locale("xx_GB").as_deref(), Some("gb"));
        assert_eq!(data.keyboard_for_locale("xx_ZZ"), None);
    }

    #[test]
    fn missing_files_leave_one_entry_lists() {
        let data = load(&[]);
        assert_eq!(values(&data.locales), ["en_US.UTF-8"]);
        assert_eq!(values(&data.keyboards), ["us"]);
        assert_eq!(values(&data.timezones), [UTC]);
        // No table, but the one layout left is named after the country.
        assert_eq!(
            data.keyboard_for_locale("en_US.UTF-8").as_deref(),
            Some("us")
        );
        assert_eq!(data.timezone_for_locale("en_US.UTF-8"), None);
    }

    #[test]
    fn search_ignores_case_and_underscores() {
        let list = vec![
            Choice {
                value: "America/New_York".into(),
                label: "America/New York".into(),
                detail: "United States".into(),
            },
            Choice {
                value: "Europe/Berlin".into(),
                label: "Europe/Berlin".into(),
                detail: "Germany".into(),
            },
        ];
        assert_eq!(search(&list, "new_york").len(), 1);
        assert_eq!(search(&list, "GERMANY")[0].value, "Europe/Berlin");
        assert_eq!(search(&list, "  ").len(), 2);
        assert!(search(&list, "Mars").is_empty());
    }

    #[test]
    fn exact_matches_come_first_then_labels_starting_with_the_query() {
        let choice = |value: &str, label: &str| Choice {
            value: value.into(),
            label: label.into(),
            detail: value.into(),
        };
        let list = vec![
            choice("by", "Belarusian"),
            choice("ru", "Russian"),
            choice("us(intl)", "English (US, intl.)"),
            choice("us", "English (US)"),
            choice("usa", "Usability test"),
        ];
        let values: Vec<String> = search(&list, "us").into_iter().map(|c| c.value).collect();
        assert_eq!(values, ["us", "usa", "by", "ru", "us(intl)"]);
    }

    #[test]
    fn equal_labels_get_their_values() {
        let mut list = vec![
            Choice {
                value: "a".into(),
                label: "Same".into(),
                detail: String::new(),
            },
            Choice {
                value: "b".into(),
                label: "Same".into(),
                detail: String::new(),
            },
            Choice {
                value: "c".into(),
                label: "Other".into(),
                detail: String::new(),
            },
        ];
        disambiguate(&mut list);
        let labels: Vec<&str> = list.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Same — a", "Same — b", "Other"]);
    }
}
