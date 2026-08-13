use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use rust_decimal::Decimal;

use crate::{
    gaeb_reader::{GaebDocument, GaebItem, GaebRow},
    model::{BillOfQuantities, Node, Position},
};

/// Liest GAEB-90-Dateien der Phasen D81, D83 und D84.
///
/// GAEB 90 verwendet 80 Zeichen breite Datensätze im DOS-Zeichensatz CP850.
/// Die Ordnungszahl wird anhand der neunstelligen OZ-Maske aus Satzart 00
/// gegliedert; dadurch bleiben unterschiedliche Strukturen wie `01.07.0006`
/// oder `02.01.01.1` erhalten.
pub fn read_gaeb_90(path: impl AsRef<Path>) -> Result<GaebDocument> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| {
        format!(
            "GAEB-90-Datei konnte nicht gelesen werden: {}",
            path.display()
        )
    })?;
    let text = decode_cp850(&bytes);
    parse_gaeb_90(
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("GAEB-90-Datei"),
        &text,
    )
}

fn parse_gaeb_90(source: &str, text: &str) -> Result<GaebDocument> {
    let records = text.lines().map(str::trim_end).collect::<Vec<_>>();
    let Some(header) = records.first() else {
        bail!("GAEB-90-Datei ist leer.");
    };
    if field(header, 0, 2) != "00" || header.chars().count() < 73 {
        bail!("Kein gültiger GAEB-90-Kopfsatz gefunden.");
    }
    let phase_field = field(header, 10, 12);
    let phase = phase_field.trim();
    if !matches!(phase, "81" | "83" | "84") {
        bail!("GAEB-90-Phase D{phase} wird noch nicht unterstützt.");
    }
    let oz_mask = field(header, 62, 71);
    if oz_mask.trim().is_empty() {
        bail!("GAEB-90-Datei enthält keine OZ-Maske.");
    }

    let mut document = GaebDocument {
        source: source.to_owned(),
        project: String::new(),
        boq: String::new(),
        exchange_phase: phase.to_owned(),
        currency: "EUR".to_owned(),
        rows: Vec::new(),
    };
    let mut active_category: Option<usize> = None;
    let mut active_item: Option<GaebItem> = None;

    for record in records.into_iter().skip(1) {
        if record.chars().count() < 2 {
            continue;
        }
        match field(record, 0, 2).as_str() {
            "01" => document.project = field(record, 2, 42).trim().to_owned(),
            "02" => document.boq = field(record, 2, 72).trim().to_owned(),
            "08" => {
                let currency_field = field(record, 2, 5);
                let currency = currency_field.trim();
                if !currency.is_empty() {
                    document.currency = currency.to_owned();
                }
            }
            "11" => {
                finish_item(&mut document.rows, &mut active_item);
                let oz = format_oz(&field(record, 2, 11), &oz_mask);
                if oz.is_empty() {
                    active_category = None;
                    continue;
                }
                let level = oz.split('.').count();
                document.rows.push(GaebRow::Category {
                    oz,
                    title: String::new(),
                    level,
                });
                active_category = Some(document.rows.len() - 1);
            }
            "12" => {
                let value_field = field(record, 2, 72);
                let value = value_field.trim();
                if let Some(GaebRow::Category { title, .. }) =
                    active_category.and_then(|index| document.rows.get_mut(index))
                {
                    append_line(title, value);
                }
            }
            "21" => {
                finish_item(&mut document.rows, &mut active_item);
                active_category = None;
                active_item = Some(GaebItem {
                    oz: format_oz(&field(record, 2, 11), &oz_mask),
                    quantity: parse_implied_decimal(&field(record, 23, 34), 3),
                    unit: field(record, 34, 38).trim().to_owned(),
                    ..GaebItem::default()
                });
            }
            "23" if phase == "84" => {
                finish_item(&mut document.rows, &mut active_item);
                active_category = None;
                active_item = Some(GaebItem {
                    oz: format_oz(&field(record, 2, 11), &oz_mask),
                    unit_price: parse_implied_decimal(&field(record, 13, 24), 3),
                    total_price: parse_implied_decimal(&field(record, 25, 37), 2),
                    ..GaebItem::default()
                });
            }
            "25" => {
                if let Some(item) = active_item.as_mut() {
                    append_line(&mut item.short_text, field(record, 2, 72).trim());
                }
            }
            "26" => {
                if let Some(item) = active_item.as_mut() {
                    append_line(&mut item.long_text, field(record, 5, 72).trim());
                }
            }
            "31" | "99" => {
                finish_item(&mut document.rows, &mut active_item);
                active_category = None;
            }
            _ => {}
        }
    }
    finish_item(&mut document.rows, &mut active_item);

    if !document
        .rows
        .iter()
        .any(|row| matches!(row, GaebRow::Item(_)))
    {
        bail!("GAEB-90-Datei enthält keine D{phase}-Positionen.");
    }
    Ok(document)
}

fn finish_item(rows: &mut Vec<GaebRow>, item: &mut Option<GaebItem>) {
    let Some(mut value) = item.take() else {
        return;
    };
    value.short_text = value.short_text.trim().to_owned();
    value.long_text = value.long_text.trim().to_owned();
    rows.push(GaebRow::Item(value));
}

fn append_line(target: &mut String, value: &str) {
    if value.is_empty() {
        if !target.is_empty() && !target.ends_with("\n\n") {
            target.push('\n');
        }
        return;
    }
    if !target.is_empty() && !target.ends_with('\n') {
        target.push('\n');
    }
    target.push_str(value);
}

fn field(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn parse_implied_decimal(value: &str, scale: u32) -> Option<Decimal> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<i64>()
        .ok()
        .map(|number| Decimal::new(number, scale))
}

fn format_oz(raw: &str, mask: &str) -> String {
    let raw = raw.as_bytes();
    let mask = mask.as_bytes();
    let mut parts = Vec::new();
    let mut index = 0usize;
    while index < raw.len().min(mask.len()) {
        let marker = mask[index];
        let mut end = index + 1;
        while end < raw.len().min(mask.len()) && mask[end] == marker {
            end += 1;
        }
        if marker != b'0' && marker != b'I' && marker != b' ' {
            let part = std::str::from_utf8(&raw[index..end])
                .unwrap_or_default()
                .trim();
            if !part.is_empty() {
                parts.push(part);
            }
        }
        index = end;
    }
    parts.join(".")
}

/// Überführt eingelesene GAEB-Daten in das gemeinsame Modell des X83-Writers.
pub fn gaeb_document_to_boq(document: &GaebDocument) -> BillOfQuantities {
    let mut boq = BillOfQuantities::new(&document.source);
    boq.project = document.project.clone();
    boq.currency = document.currency.clone();

    for row in &document.rows {
        match row {
            GaebRow::Category { oz, title, .. } => {
                let parts = oz.split('.').collect::<Vec<_>>();
                let node = ensure_node(&mut boq.roots, &parts, 0);
                if !title.trim().is_empty() {
                    node.title = title.replace('\n', " ");
                }
            }
            GaebRow::Item(item) => {
                let mut parts = item.oz.split('.').collect::<Vec<_>>();
                if parts.len() < 2 {
                    continue;
                }
                parts.pop();
                let parent = ensure_node(&mut boq.roots, &parts, 0);
                parent.positions.push(Position {
                    oz: item.oz.clone(),
                    quantity: item.quantity,
                    unit: (!item.unit.is_empty()).then(|| item.unit.clone()),
                    unit_price: item.unit_price,
                    total_price: item.total_price,
                    short_text: item.short_text.replace('\n', " "),
                    long_text: item.long_text.clone(),
                    ..Position::default()
                });
            }
        }
    }
    boq
}

fn ensure_node<'a>(nodes: &'a mut Vec<Node>, parts: &[&str], index: usize) -> &'a mut Node {
    let oz = parts[..=index].join(".");
    let node_index = nodes
        .iter()
        .position(|node| node.oz == oz)
        .unwrap_or_else(|| {
            nodes.push(Node {
                oz: oz.clone(),
                level: index + 1,
                ..Node::default()
            });
            nodes.len() - 1
        });
    if index + 1 == parts.len() {
        &mut nodes[node_index]
    } else {
        ensure_node(&mut nodes[node_index].children, parts, index + 1)
    }
}

fn decode_cp850(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| {
            if *byte < 0x80 {
                char::from(*byte)
            } else {
                CP850_HIGH[usize::from(*byte - 0x80)]
            }
        })
        .collect()
}

const CP850_HIGH: [char; 128] = [
    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}', '\u{00E4}', '\u{00E0}', '\u{00E5}', '\u{00E7}',
    '\u{00EA}', '\u{00EB}', '\u{00E8}', '\u{00EF}', '\u{00EE}', '\u{00EC}', '\u{00C4}', '\u{00C5}',
    '\u{00C9}', '\u{00E6}', '\u{00C6}', '\u{00F4}', '\u{00F6}', '\u{00F2}', '\u{00FB}', '\u{00F9}',
    '\u{00FF}', '\u{00D6}', '\u{00DC}', '\u{00F8}', '\u{00A3}', '\u{00D8}', '\u{00D7}', '\u{0192}',
    '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}', '\u{00F1}', '\u{00D1}', '\u{00AA}', '\u{00BA}',
    '\u{00BF}', '\u{00AE}', '\u{00AC}', '\u{00BD}', '\u{00BC}', '\u{00A1}', '\u{00AB}', '\u{00BB}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{00C1}', '\u{00C2}', '\u{00C0}',
    '\u{00A9}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255D}', '\u{00A2}', '\u{00A5}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252C}', '\u{251C}', '\u{2500}', '\u{253C}', '\u{00E3}', '\u{00C3}',
    '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256C}', '\u{00A4}',
    '\u{00F0}', '\u{00D0}', '\u{00CA}', '\u{00CB}', '\u{00C8}', '\u{0131}', '\u{00CD}', '\u{00CE}',
    '\u{00CF}', '\u{2518}', '\u{250C}', '\u{2588}', '\u{2584}', '\u{00A6}', '\u{00CC}', '\u{2580}',
    '\u{00D3}', '\u{00DF}', '\u{00D4}', '\u{00D2}', '\u{00F5}', '\u{00D5}', '\u{00B5}', '\u{00FE}',
    '\u{00DE}', '\u{00DA}', '\u{00DB}', '\u{00D9}', '\u{00FD}', '\u{00DD}', '\u{00AF}', '\u{00B4}',
    '\u{00AD}', '\u{00B1}', '\u{2017}', '\u{00BE}', '\u{00B6}', '\u{00A7}', '\u{00F7}', '\u{00B8}',
    '\u{00B0}', '\u{00A8}', '\u{00B7}', '\u{00B9}', '\u{00B3}', '\u{00B2}', '\u{25A0}', '\u{00A0}',
];

#[cfg(test)]
mod tests {
    use super::*;

    fn record(value: &str, sequence: usize) -> String {
        format!("{value:<74}{sequence:06}")
    }

    #[test]
    fn parses_d83_structure_text_quantity_and_cp850() {
        let data = [
            record(
                "00        83L                                                 112233PP090",
                1,
            ),
            record("01Testprojekt", 2),
            record("08EUR   Euro", 3),
            record("11010203   N    Titel", 4),
            record("12Trockenbau", 5),
            record("21010203 4 NNN         00000012500m2", 6),
            record("25GK-Wand 100mm F90", 7),
            record("26   Liefern und montieren.", 8),
            record("99", 9),
        ]
        .join("\r\n");
        let document = parse_gaeb_90("test.d83", &data).unwrap();
        assert_eq!(document.exchange_phase, "83");
        assert!(matches!(
            &document.rows[0],
            GaebRow::Category { oz, title, .. } if oz == "01.02.03" && title == "Trockenbau"
        ));
        let GaebRow::Item(item) = &document.rows[1] else {
            panic!("Position erwartet")
        };
        assert_eq!(item.oz, "01.02.03.4");
        assert_eq!(item.quantity, Some(Decimal::new(12500, 3)));
        assert_eq!(item.unit, "m2");
        assert_eq!(item.short_text, "GK-Wand 100mm F90");
        assert_eq!(item.long_text, "Liefern und montieren.");
    }

    #[test]
    fn parses_d81_four_digit_position_number() {
        let data = [
            record(
                "00        81L                                                 1122PPPPI90",
                1,
            ),
            record("110107     N    Abschnitt", 2),
            record("12Fenster", 3),
            record("2101070006 NNN         00000001000Psch", 4),
            record("25Windfang", 5),
            record("99", 6),
        ]
        .join("\r\n");
        let document = parse_gaeb_90("test.d81", &data).unwrap();
        let GaebRow::Item(item) = &document.rows[1] else {
            panic!("Position erwartet")
        };
        assert_eq!(item.oz, "01.07.0006");
        assert_eq!(item.quantity, Some(Decimal::ONE));
        assert_eq!(item.unit, "Psch");
    }

    #[test]
    fn converts_document_for_x83_writer() {
        let document = GaebDocument {
            source: "test.d83".into(),
            project: "Projekt".into(),
            boq: "LV".into(),
            exchange_phase: "83".into(),
            currency: "EUR".into(),
            rows: vec![
                GaebRow::Category {
                    oz: "01".into(),
                    title: "Los".into(),
                    level: 1,
                },
                GaebRow::Item(GaebItem {
                    oz: "01.10".into(),
                    short_text: "Leistung".into(),
                    quantity: Some(Decimal::ONE),
                    unit: "St".into(),
                    ..GaebItem::default()
                }),
            ],
        };
        let boq = gaeb_document_to_boq(&document);
        assert_eq!(boq.roots[0].title, "Los");
        assert_eq!(boq.roots[0].positions[0].oz, "01.10");
    }
}
