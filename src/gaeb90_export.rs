use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use chrono::Local;
use rust_decimal::Decimal;

use crate::{GaebDocument, GaebRow};

/// Schreibt eine GAEB-90-D84-Angebotsabgabe mit OZ, EP und GB.
pub fn write_d84(document: &GaebDocument, path: impl AsRef<Path>) -> Result<()> {
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
        bail!("Die GAEB-Datei enthält keine Positionen für eine D84.");
    }
    if items
        .iter()
        .any(|item| item.unit_price.is_none() || item.total_price.is_none())
    {
        bail!("D84 benötigt für jede Position einen Einheits- und Gesamtpreis.");
    }

    let part_lengths = oz_part_lengths(&items)?;
    let oz_mask = oz_mask(&part_lengths);
    let mut records = Vec::new();
    let mut header = vec![b' '; 74];
    put(&mut header, 0, "00");
    put(&mut header, 10, "84");
    put(&mut header, 62, &oz_mask);
    put(&mut header, 71, "90");
    records.push(header);

    let now = Local::now();
    records.push(record_with(
        74,
        &[
            (0, "01"),
            (
                2,
                truncate(
                    if document.project.trim().is_empty() {
                        "GAEB-Angebot"
                    } else {
                        &document.project
                    },
                    40,
                ),
            ),
            (42, &now.format("%d.%m.%y").to_string()),
            (68, "X"),
        ],
    ));
    records.push(record_with(
        74,
        &[(0, "02"), (2, truncate(&document.boq, 70))],
    ));
    records.push(record_with(
        74,
        &[(0, "08"), (2, currency(document)), (6, currency(document))],
    ));

    for item in &items {
        let compact = compact_oz(&item.oz, &part_lengths)?;
        let ep = implied_price(item.unit_price.expect("validated"), 3, 11)?;
        let gb = implied_price(item.total_price.expect("validated"), 2, 12)?;
        records.push(record_with(
            74,
            &[(0, "23"), (2, &compact), (13, &ep), (25, &gb)],
        ));
    }

    records.push(record_with(
        74,
        &[(0, "99"), (2, &format!("{:07}", items.len()))],
    ));
    let mut output = Vec::with_capacity(records.len() * 82);
    for (index, mut record) in records.into_iter().enumerate() {
        let sequence = format!("{:06}", index + 1);
        record.extend_from_slice(sequence.as_bytes());
        output.extend(encode_cp850(
            &String::from_utf8(record).expect("ASCII record"),
        ));
        output.extend_from_slice(b"\r\n");
    }
    fs::write(path, output)
        .with_context(|| format!("D84 konnte nicht geschrieben werden: {}", path.display()))?;
    Ok(())
}

fn oz_part_lengths(items: &[&crate::GaebItem]) -> Result<Vec<usize>> {
    let first = items[0].oz.split('.').map(str::len).collect::<Vec<_>>();
    if first.is_empty() || first.len() > 6 || first.iter().sum::<usize>() > 9 {
        bail!("Die OZ-Struktur passt nicht in die neunstellige GAEB-90-OZ-Maske.");
    }
    for item in items.iter().skip(1) {
        let lengths = item.oz.split('.').map(str::len).collect::<Vec<_>>();
        if lengths != first {
            bail!("D84 erfordert eine einheitliche numerische OZ-Struktur.");
        }
    }
    if items.iter().any(|item| {
        !item
            .oz
            .split('.')
            .all(|part| part.bytes().all(|b| b.is_ascii_digit()))
    }) {
        bail!("D84 unterstützt in diesem Export nur numerische Ordnungszahlen.");
    }
    Ok(first)
}

fn oz_mask(lengths: &[usize]) -> String {
    let markers = if lengths.len() == 6 {
        ['1', '2', '3', '4', 'P', 'I']
    } else {
        ['1', '2', '3', '4', '5', 'P']
    };
    let mut mask = String::new();
    for (index, length) in lengths.iter().enumerate() {
        mask.extend(std::iter::repeat_n(
            markers[index + 6 - lengths.len()],
            *length,
        ));
    }
    format!("{mask:<9}")
}

fn compact_oz(value: &str, lengths: &[usize]) -> Result<String> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != lengths.len()
        || parts
            .iter()
            .zip(lengths)
            .any(|(part, length)| part.len() != *length)
    {
        bail!("OZ {value} entspricht nicht der ermittelten GAEB-90-OZ-Maske.");
    }
    Ok(format!("{:<9}", parts.concat()))
}

fn implied_price(value: Decimal, scale: usize, width: usize) -> Result<String> {
    let negative = value.is_sign_negative();
    let digits = format!("{:.*}", scale, value.abs())
        .replace('.', "")
        .replace(',', "");
    let available = width - usize::from(negative);
    if digits.len() > available {
        bail!("Preis {value} überschreitet die Feldbreite des GAEB-90-Formats.");
    }
    Ok(if negative {
        format!("-{:0>available$}", digits)
    } else {
        format!("{:0>width$}", digits)
    })
}

fn record_with(length: usize, fields: &[(usize, &str)]) -> Vec<u8> {
    let mut record = vec![b' '; length];
    for (offset, value) in fields {
        put(&mut record, *offset, value);
    }
    record
}
fn put(record: &mut [u8], offset: usize, value: &str) {
    for (target, source) in record.iter_mut().skip(offset).zip(value.as_bytes()) {
        *target = *source;
    }
}
fn truncate(value: &str, length: usize) -> &str {
    value.get(..value.len().min(length)).unwrap_or("")
}
fn currency(document: &GaebDocument) -> &str {
    if document.currency.trim().is_empty() {
        "EUR"
    } else {
        &document.currency
    }
}

fn encode_cp850(value: &str) -> Vec<u8> {
    value
        .chars()
        .map(|character| match character {
            'ä' => 0x84,
            'ö' => 0x94,
            'ü' => 0x81,
            'Ä' => 0x8e,
            'Ö' => 0x99,
            'Ü' => 0x9a,
            'ß' => 0xe1,
            'é' => 0x82,
            '€' => b'E',
            character if character.is_ascii() => character as u8,
            _ => b'?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GaebItem;

    #[test]
    fn writes_80_column_d84_and_reads_prices_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("angebot.d84");
        let document = GaebDocument {
            source: "angebot.x84".into(),
            project: "Projekt".into(),
            boq: "LV".into(),
            exchange_phase: "84".into(),
            currency: "EUR".into(),
            rows: vec![GaebRow::Item(GaebItem {
                oz: "01.03.001".into(),
                unit_price: Some(Decimal::new(5580, 2)),
                total_price: Some(Decimal::new(1132740, 2)),
                ..Default::default()
            })],
        };
        write_d84(&document, &path).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .all(|line| line.trim_ascii_end().len() == 80));
        let parsed = crate::read_gaeb_90(&path).unwrap();
        let GaebRow::Item(item) = &parsed.rows[0] else {
            panic!("Position erwartet")
        };
        assert_eq!(item.oz, "01.03.001");
        assert_eq!(item.unit_price, Some(Decimal::new(55800, 3)));
        assert_eq!(item.total_price, Some(Decimal::new(1132740, 2)));
    }
}
