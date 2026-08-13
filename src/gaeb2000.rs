use std::{borrow::Cow, fs, path::Path, str::FromStr};

use anyhow::{bail, Context, Result};
use quick_xml::escape::unescape;
use rust_decimal::Decimal;

use crate::gaeb_reader::{GaebDocument, GaebItem, GaebRow};

/// Liest GAEB DA 2000 der Phasen P81, P83 und P84.
pub fn read_gaeb_2000(path: impl AsRef<Path>) -> Result<GaebDocument> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| {
        format!(
            "GAEB-DA-2000-Datei konnte nicht gelesen werden: {}",
            path.display()
        )
    })?;
    parse_gaeb_2000(
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("GAEB-DA-2000-Datei"),
        &decode_windows_1252(&bytes),
    )
}

fn parse_gaeb_2000(source: &str, text: &str) -> Result<GaebDocument> {
    if !text.lines().any(|line| line.trim() == "#begin[GAEB]") {
        bail!("Kein gültiger GAEB-DA-2000-Kopf gefunden.");
    }
    let mut document = GaebDocument {
        source: source.to_owned(),
        project: String::new(),
        boq: String::new(),
        exchange_phase: String::new(),
        currency: "EUR".to_owned(),
        rows: Vec::new(),
    };
    let mut sections = Vec::<String>::new();
    let mut level_lengths = Vec::<usize>::new();
    let mut glied_type = String::new();
    let mut glied_length = None;
    let mut active_category = None;
    let mut item: Option<GaebItem> = None;
    let mut multiline: Option<(String, String)> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if let Some((tag, value)) = multiline.as_mut() {
            if let Some(last_line) = trimmed.strip_suffix("[end]") {
                if !last_line.is_empty() {
                    append_multiline(value, last_line);
                }
                apply_value(
                    &mut document,
                    &sections,
                    tag,
                    value,
                    &level_lengths,
                    &mut glied_type,
                    &mut glied_length,
                    &mut active_category,
                    &mut item,
                );
                multiline = None;
            } else {
                append_multiline(value, line.trim_start());
            }
            continue;
        }

        if let Some(section) = block_name(trimmed, "#begin[") {
            if section == "Position" {
                finish_item(&mut document, &mut item);
                item = Some(GaebItem::default());
            } else if section == "LVGlied" {
                glied_type.clear();
                glied_length = None;
            }
            sections.push(section.to_owned());
            continue;
        }
        if let Some(section) = block_name(trimmed, "#end[") {
            if section == "Position" {
                finish_item(&mut document, &mut item);
            } else if section == "LVGlied" && matches!(glied_type.as_str(), "LVStufe" | "Position")
            {
                if let Some(length) = glied_length {
                    level_lengths.push(length);
                }
            }
            if let Some(index) = sections.iter().rposition(|value| value == section) {
                sections.truncate(index);
            }
            continue;
        }

        let Some((tag, rest)) = field_start(trimmed) else {
            continue;
        };
        if let Some(value) = rest.strip_suffix("[end]") {
            apply_value(
                &mut document,
                &sections,
                tag,
                value,
                &level_lengths,
                &mut glied_type,
                &mut glied_length,
                &mut active_category,
                &mut item,
            );
        } else {
            multiline = Some((tag.to_owned(), rest.to_owned()));
        }
    }
    finish_item(&mut document, &mut item);

    if !matches!(document.exchange_phase.as_str(), "81" | "83" | "84") {
        bail!(
            "GAEB DA 2000 Phase P{} wird noch nicht unterstützt.",
            document.exchange_phase
        );
    }
    if matches!(document.exchange_phase.as_str(), "83" | "84")
        && !document
            .rows
            .iter()
            .any(|row| matches!(row, GaebRow::Item(_)))
    {
        bail!("GAEB-DA-2000-P83-Datei enthält keine Positionen.");
    }
    Ok(document)
}

#[allow(clippy::too_many_arguments)]
fn apply_value(
    document: &mut GaebDocument,
    sections: &[String],
    tag: &str,
    raw_value: &str,
    level_lengths: &[usize],
    glied_type: &mut String,
    glied_length: &mut Option<usize>,
    active_category: &mut Option<usize>,
    item: &mut Option<GaebItem>,
) {
    let value = decode_entities(raw_value.trim());
    let in_section = |name: &str| sections.iter().any(|section| section == name);
    // ZuschlPosition ist ein eingebetteter Bezugs-/Zuschlagsdatensatz der
    // Hauptposition und besitzt regelmäßig keine eigene Menge oder Einheit.
    let in_position = in_section("Position") && !in_section("ZuschlPosition");
    match (tag, in_position) {
        ("DP", _) => document.exchange_phase = value.trim().to_owned(),
        ("Wae", false) if !value.trim().is_empty() => document.currency = value.trim().to_owned(),
        ("Name", false) if in_section("PrjInfo") => document.project = value.trim().to_owned(),
        ("Name", false) if in_section("LVInfo") => document.boq = value.trim().to_owned(),
        ("Typ", false) if in_section("LVGlied") => *glied_type = value.trim().to_owned(),
        ("Laenge", false) if in_section("LVGlied") => *glied_length = value.trim().parse().ok(),
        ("OZ", true) if sections.last().is_some_and(|value| value == "Position") => {
            if let Some(position) = item.as_mut() {
                position.oz = format_compact_oz(value.trim(), level_lengths);
            }
        }
        ("Kurztext", true) => {
            if let Some(position) = item.as_mut() {
                position.short_text = value.trim().to_owned();
            }
        }
        ("Langtext", true) => {
            if let Some(position) = item.as_mut() {
                position.long_text = value.trim().to_owned();
            }
        }
        ("ME", true) => {
            if let Some(position) = item.as_mut() {
                position.unit = value.trim().to_owned();
            }
        }
        ("Menge", true) => {
            if let Some(position) = item.as_mut() {
                position.quantity = parse_decimal(&value);
            }
        }
        ("EP", true) => {
            if let Some(position) = item.as_mut() {
                position.unit_price = parse_decimal(&value);
            }
        }
        ("GB", true) => {
            if let Some(position) = item.as_mut() {
                position.total_price = parse_decimal(&value);
            }
        }
        ("OZ", false) if sections.last().is_some_and(|value| value == "LVBereich") => {
            let oz = format_compact_oz(value.trim(), level_lengths);
            let level = sections
                .iter()
                .filter(|section| section.as_str() == "LVBereich")
                .count();
            document.rows.push(GaebRow::Category {
                oz,
                title: String::new(),
                level,
            });
            *active_category = Some(document.rows.len() - 1);
        }
        ("Bez", false) if sections.last().is_some_and(|value| value == "LVBereich") => {
            if let Some(GaebRow::Category { title, .. }) =
                active_category.and_then(|index| document.rows.get_mut(index))
            {
                *title = value.trim().to_owned();
            }
        }
        _ => {}
    }
}

fn finish_item(document: &mut GaebDocument, item: &mut Option<GaebItem>) {
    if let Some(value) = item.take() {
        if !value.oz.is_empty() {
            document.rows.push(GaebRow::Item(value));
        }
    }
}

fn append_multiline(target: &mut String, value: &str) {
    if !target.is_empty() {
        target.push('\n');
    }
    target.push_str(value);
}

fn block_name<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)?.strip_suffix(']')
}

fn field_start(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    Some((&rest[..end], &rest[end + 1..]))
}

fn format_compact_oz(raw: &str, lengths: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut offset = 0usize;
    for length in lengths {
        let part = raw
            .get(offset..offset.saturating_add(*length))
            .unwrap_or_default()
            .trim();
        if !part.is_empty() {
            parts.push(part);
        }
        offset = offset.saturating_add(*length);
        if offset >= raw.len() {
            break;
        }
    }
    if offset < raw.len() {
        let index = raw[offset..].trim();
        if !index.is_empty() {
            parts.push(index);
        }
    }
    if parts.is_empty() {
        raw.to_owned()
    } else {
        parts.join(".")
    }
}

fn parse_decimal(value: &str) -> Option<Decimal> {
    Decimal::from_str(&value.trim().replace('.', "").replace(',', ".")).ok()
}

fn decode_entities(value: &str) -> Cow<'_, str> {
    unescape(value).unwrap_or(Cow::Borrowed(value))
}

fn decode_windows_1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| match *byte {
            0x00..=0x7f | 0xa0..=0xff => char::from(*byte),
            0x80..=0x9f => WINDOWS_1252_CONTROLS[usize::from(*byte - 0x80)],
        })
        .collect()
}

const WINDOWS_1252_CONTROLS: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_p83_category_position_and_text() {
        let text = r#"#begin[GAEB]
 #begin[PrjInfo]
  [Name]Projekt 1[end]
  [Wae]EUR[end]
 #end[PrjInfo]
 #begin[Vergabe]
  [DP]83[end]
  #begin[LV]
   #begin[LVInfo]
    [Name]Trockenbau[end]
    #begin[LVGlied]
     [Typ]LVStufe[end]
     [Laenge]2[end]
    #end[LVGlied]
    #begin[LVGlied]
     [Typ]LVStufe[end]
     [Laenge]2[end]
    #end[LVGlied]
    #begin[LVGlied]
     [Typ]Position[end]
     [Laenge]4[end]
    #end[LVGlied]
   #end[LVInfo]
   #begin[LVBereich]
    [OZ]0101[end]
    [Bez]Wände[end]
    #begin[Position]
     [OZ]01010010[end]
     #begin[Beschreibung]
      [Langtext]
      Liefern &#38; montieren
      [end]
      [Kurztext]GK-Wand 100 mm[end]
     #end[Beschreibung]
     [ME]m²[end]
     [Menge]12,500[end]
    #end[Position]
   #end[LVBereich]
  #end[LV]
 #end[Vergabe]
#end[GAEB]"#;
        let document = parse_gaeb_2000("test.p83", text).unwrap();
        assert_eq!(document.project, "Projekt 1");
        assert_eq!(document.boq, "Trockenbau");
        assert!(matches!(
            &document.rows[0],
            GaebRow::Category { oz, title, .. } if oz == "01.01" && title == "Wände"
        ));
        let GaebRow::Item(item) = &document.rows[1] else {
            panic!("Position erwartet")
        };
        assert_eq!(item.oz, "01.01.0010");
        assert_eq!(item.short_text, "GK-Wand 100 mm");
        assert_eq!(item.long_text, "Liefern & montieren");
        assert_eq!(item.quantity, Some(Decimal::new(12500, 3)));
        assert_eq!(item.unit, "m²");
    }
}
