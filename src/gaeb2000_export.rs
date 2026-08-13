use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use chrono::Local;
use rust_decimal::Decimal;

use crate::{GaebDocument, GaebRow, CONVERTER_NAME};

/// Schreibt eine kompakte GAEB-DA-2000-P84-Angebotsabgabe.
///
/// P84 überträgt zur unveränderten OZ-Struktur die Angebotspreise. Texte,
/// Mengen und Einheiten verbleiben in der zugehörigen Aufforderungsdatei.
pub fn write_p84(document: &GaebDocument, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let items = document
        .rows
        .iter()
        .filter_map(|row| match row {
            GaebRow::Item(item) => Some(item),
            GaebRow::Category { .. } => None,
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        bail!("Die GAEB-Datei enthält keine Positionen für eine P84.");
    }
    if items
        .iter()
        .any(|item| item.unit_price.is_none() || item.total_price.is_none())
    {
        bail!("P84 benötigt für jede Position einen Einheits- und Gesamtpreis.");
    }

    let level_lengths = hierarchy_lengths(document);
    let now = Local::now();
    let mut output = String::new();
    line(&mut output, 0, "#begin[GAEB]");
    line(&mut output, 1, "#begin[GAEBInfo]");
    field(&mut output, 2, "Version", "1.2");
    field(&mut output, 2, "VersMon", "3");
    field(&mut output, 2, "VersJahr", "2002");
    field(&mut output, 2, "Datum", &now.format("%d.%m.%Y").to_string());
    field(&mut output, 2, "Uhrzeit", &now.format("%H:%M").to_string());
    field(&mut output, 2, "ProgSystem", CONVERTER_NAME);
    field(&mut output, 2, "ProgName", CONVERTER_NAME);
    field(&mut output, 2, "Zeichensatz", "ANSI");
    line(&mut output, 1, "#end[GAEBInfo]");
    line(&mut output, 1, "#begin[Vergabe]");
    field(&mut output, 2, "DP", "84");
    line(&mut output, 2, "#begin[VergabeInfo]");
    field(&mut output, 3, "Wae", currency(document));
    field(&mut output, 3, "WaeBez", currency(document));
    line(&mut output, 2, "#end[VergabeInfo]");
    line(&mut output, 2, "#begin[AG]");
    line(&mut output, 2, "#end[AG]");
    line(&mut output, 2, "#begin[AN]");
    line(&mut output, 2, "#end[AN]");
    line(&mut output, 2, "#begin[LV]");
    line(&mut output, 3, "#begin[LVInfo]");
    field(
        &mut output,
        4,
        "Name",
        text_or(&document.boq, "Leistungsverzeichnis"),
    );
    field(
        &mut output,
        4,
        "Bez",
        text_or(&document.project, &document.boq),
    );
    field(&mut output, 4, "Datum", &now.format("%d.%m.%Y").to_string());
    field(&mut output, 4, "KurzLang", "1");
    for length in &level_lengths {
        line(&mut output, 4, "#begin[LVGlied]");
        field(&mut output, 5, "Typ", "LVStufe");
        field(&mut output, 5, "Laenge", &length.to_string());
        line(&mut output, 4, "#end[LVGlied]");
    }
    line(&mut output, 4, "#begin[LVGlied]");
    field(&mut output, 5, "Typ", "Position");
    field(
        &mut output,
        5,
        "Laenge",
        &position_length(document).to_string(),
    );
    line(&mut output, 4, "#end[LVGlied]");
    line(&mut output, 3, "#end[LVInfo]");

    let mut open_categories = 0usize;
    for row in &document.rows {
        match row {
            GaebRow::Category { oz, title, level } => {
                while open_categories >= *level && open_categories > 0 {
                    line(&mut output, 2 + open_categories, "#end[LVBereich]");
                    open_categories -= 1;
                }
                line(&mut output, 3 + open_categories, "#begin[LVBereich]");
                field(&mut output, 4 + open_categories, "OZ", &compact_oz(oz));
                field(&mut output, 4 + open_categories, "Bez", title);
                open_categories += 1;
            }
            GaebRow::Item(item) => {
                line(&mut output, 3 + open_categories, "#begin[Position]");
                field(
                    &mut output,
                    4 + open_categories,
                    "OZ",
                    &compact_oz(&item.oz),
                );
                field(
                    &mut output,
                    4 + open_categories,
                    "EP",
                    &decimal(item.unit_price.expect("validated"), 3),
                );
                field(
                    &mut output,
                    4 + open_categories,
                    "GB",
                    &decimal(item.total_price.expect("validated"), 2),
                );
                line(&mut output, 3 + open_categories, "#end[Position]");
            }
        }
    }
    while open_categories > 0 {
        line(&mut output, 2 + open_categories, "#end[LVBereich]");
        open_categories -= 1;
    }
    line(&mut output, 2, "#end[LV]");
    line(&mut output, 1, "#end[Vergabe]");
    line(&mut output, 0, "#end[GAEB]");

    fs::write(path, encode_windows_1252(&output))
        .with_context(|| format!("P84 konnte nicht geschrieben werden: {}", path.display()))?;
    Ok(())
}

fn hierarchy_lengths(document: &GaebDocument) -> Vec<usize> {
    let depth = document
        .rows
        .iter()
        .filter_map(|row| match row {
            GaebRow::Category { level, .. } => Some(*level),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    (0..depth)
        .map(|index| {
            document
                .rows
                .iter()
                .filter_map(|row| match row {
                    GaebRow::Category { oz, .. } => oz.split('.').nth(index).map(str::len),
                    GaebRow::Item(item) => item.oz.split('.').nth(index).map(str::len),
                })
                .max()
                .unwrap_or(1)
        })
        .collect()
}

fn position_length(document: &GaebDocument) -> usize {
    document
        .rows
        .iter()
        .filter_map(|row| match row {
            GaebRow::Item(item) => item.oz.split('.').next_back().map(str::len),
            _ => None,
        })
        .max()
        .unwrap_or(1)
}

fn compact_oz(value: &str) -> String {
    value.split('.').collect()
}
fn currency(document: &GaebDocument) -> &str {
    text_or(&document.currency, "EUR")
}
fn text_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}
fn decimal(value: Decimal, scale: u32) -> String {
    format!("{:.*}", scale as usize, value).replace('.', ",")
}
fn line(output: &mut String, indent: usize, value: &str) {
    output.push_str(&" ".repeat(indent));
    output.push_str(value);
    output.push_str("\r\n");
}
fn field(output: &mut String, indent: usize, name: &str, value: &str) {
    line(output, indent, &format!("[{name}]{}[end]", escape(value)));
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&#38;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
}

fn encode_windows_1252(value: &str) -> Vec<u8> {
    value
        .chars()
        .map(|character| match character {
            '\u{20AC}' => 0x80,
            '\u{201A}' => 0x82,
            '\u{0192}' => 0x83,
            '\u{201E}' => 0x84,
            '\u{2026}' => 0x85,
            '\u{2020}' => 0x86,
            '\u{2021}' => 0x87,
            '\u{02C6}' => 0x88,
            '\u{2030}' => 0x89,
            '\u{0160}' => 0x8a,
            '\u{2039}' => 0x8b,
            '\u{0152}' => 0x8c,
            '\u{017D}' => 0x8e,
            '\u{2018}' => 0x91,
            '\u{2019}' => 0x92,
            '\u{201C}' => 0x93,
            '\u{201D}' => 0x94,
            '\u{2022}' => 0x95,
            '\u{2013}' => 0x96,
            '\u{2014}' => 0x97,
            '\u{02DC}' => 0x98,
            '\u{2122}' => 0x99,
            '\u{0161}' => 0x9a,
            '\u{203A}' => 0x9b,
            '\u{0153}' => 0x9c,
            '\u{017E}' => 0x9e,
            '\u{0178}' => 0x9f,
            character if u32::from(character) <= 0xff => character as u8,
            _ => b'?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GaebItem;

    #[test]
    fn writes_and_reads_p84_prices_and_oz() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("angebot.p84");
        let document = GaebDocument {
            source: "angebot.x84".into(),
            project: "Projekt".into(),
            boq: "LV".into(),
            exchange_phase: "84".into(),
            currency: "EUR".into(),
            rows: vec![
                GaebRow::Category {
                    oz: "01.03".into(),
                    title: "Titel".into(),
                    level: 2,
                },
                GaebRow::Item(GaebItem {
                    oz: "01.03.001".into(),
                    unit_price: Some(Decimal::new(5580, 2)),
                    total_price: Some(Decimal::new(1132740, 2)),
                    ..Default::default()
                }),
            ],
        };
        write_p84(&document, &path).unwrap();
        let parsed = crate::read_gaeb_2000(&path).unwrap();
        assert_eq!(parsed.exchange_phase, "84");
        let GaebRow::Item(item) = &parsed.rows[1] else {
            panic!("Position erwartet")
        };
        assert_eq!(item.oz, "01.03.001");
        assert_eq!(item.unit_price, Some(Decimal::new(5580, 2)));
        assert_eq!(item.total_price, Some(Decimal::new(1132740, 2)));
    }
}
