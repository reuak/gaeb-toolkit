//! Parser for compact, unpriced LVs with "1.1 Beschreibung" followed by "65 m² ____ ____".
use std::{collections::HashMap, str::FromStr};
use anyhow::Result;
use regex::Regex;
use rust_decimal::Decimal;
use crate::model::{BillOfQuantities, Node, Position};

/// Only activates for the recognizable compact layout. Other PDFs retain the existing parser.
pub fn looks_like_compact_lv(text: &str) -> bool {
    text.contains("Pos. / Leistungsbeschreibung")
        && text.contains("Menge / Einheit")
        && text.lines().any(|l| l.trim_start().starts_with("1.1 "))
}

pub fn parse_compact_lv(source: &str, text: &str) -> Result<BillOfQuantities> {
    let heading = Regex::new(r"^(\d{1,2})\s+(\S.+)$")?;
    let item = Regex::new(r"^(\d{1,2})\.(\d{1,3})\s+(\S.+)$")?;
    let quantity = Regex::new(r"^(\d+(?:[.,]\d+)?)\s+(m²|m2|lfdm|St|psch|h)\s+(?:_+|nur\s+EP)")?;
    let mut boq = BillOfQuantities::new(source);
    let mut current_chapter: Option<usize> = None;
    let mut current_item: Option<Position> = None;
    let mut current_original = String::new();
    let mut preamble = Vec::new();
    let mut started = false;
    let mut numbering: HashMap<usize, usize> = HashMap::new();
    let mut original_to_exported: Vec<(String, String)> = Vec::new();

    for (page_index, page) in text.split('\u{000c}').enumerate() {
        let page_number = page_index + 1;
        for raw in page.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with("Leistungsverzeichnis -")
                || line.starts_with("Bolat Natursteine GmbH")
                || line.starts_with("Dammstraße ")
                || line.starts_with("Tel. ")
                || line.starts_with("Pos. / Leistungsbeschreibung")
                || line == "Zusammenstellung"
                || line.starts_with("Gesamtsumme netto") {
                continue;
            }
            if line.starts_with("Bauvorhaben:") && boq.project.is_empty() {
                boq.project = line.trim_start_matches("Bauvorhaben:").trim().to_owned();
            }
            if let Some(caps) = heading.captures(line) {
                let chapter_num: usize = caps[1].parse()?;
                if (1..=99).contains(&chapter_num) && !started || (started && current_item.is_some() && chapter_num <= 9 && line.len() < 100 && !line.contains(" EUR")) {
                    flush_item(&mut boq, current_chapter, &mut current_item);
                    started = true;
                    let chapter = Node {
                        oz: format!("{chapter_num:02}"),
                        title: caps[2].to_owned(),
                        level: 1,
                        page: Some(page_number),
                        ..Node::default()
                    };
                    boq.roots.push(chapter);
                    current_chapter = Some(boq.roots.len() - 1);
                    continue;
                }
            }
            if let Some(caps) = item.captures(line) {
                let chapter: usize = caps[1].parse()?;
                if let Some(index) = current_chapter {
                    if boq.roots[index].oz == format!("{chapter:02}") {
                        flush_item(&mut boq, current_chapter, &mut current_item);
                        let sequence = numbering.entry(chapter).or_insert(0);
                        *sequence += 1;
                        let original = format!("{}.{}", &caps[1], &caps[2]);
                        let exported = format!("{chapter:02}.{:03}", sequence);
                        original_to_exported.push((original.clone(), exported.clone()));
                        current_original = original;
                        current_item = Some(Position {
                            oz: exported,
                            short_text: caps[3].to_owned(),
                            page_from: Some(page_number),
                            page_to: Some(page_number),
                            ..Position::default()
                        });
                        continue;
                    }
                }
            }
            if let Some(pos) = current_item.as_mut() {
                if let Some(caps) = quantity.captures(line) {
                    let number = caps[1].replace(',', ".");
                    pos.quantity = Decimal::from_str(&number).ok();
                    pos.unit = Some(caps[2].to_owned());
                    pos.price_only = line.contains("nur EP");
                    pos.provisional = pos.price_only;
                    pos.page_to = Some(page_number);
                    continue;
                }
                // Do not pollute item descriptions with page headers or totals.
                if !line.starts_with("Seite ") && !line.contains("________________ EUR") {
                    if !pos.long_text.is_empty() { pos.long_text.push('\n'); }
                    pos.long_text.push_str(line);
                }
            } else if !started {
                preamble.push(line.to_owned());
            }
        }
    }
    flush_item(&mut boq, current_chapter, &mut current_item);
    boq.preamble = preamble.join("\n");
    // Original item numbers are retained in the descriptive text wherever remapping was needed.
    for (original, exported) in original_to_exported {
        let expected = original.split('.').last().and_then(|v| v.parse::<usize>().ok());
        let actual = exported.split('.').last().and_then(|v| v.parse::<usize>().ok());
        if expected != actual {
            boq.warnings.push(format!("OZ {original} wird wegen doppelter/verschobener Nummerierung als {exported} exportiert."));
            for root in &mut boq.roots {
                if let Some(pos) = root.positions.iter_mut().find(|pos| pos.oz == exported) {
                    pos.long_text = format!("Original-OZ im PDF: {original}\n{}", pos.long_text);
                }
            }
        }
    }
    if boq.roots.is_empty() {
        anyhow::bail!("Keine Kapitel des kompakten LV-Formats erkannt");
    }
    Ok(boq)
}

fn flush_item(boq: &mut BillOfQuantities, chapter: Option<usize>, current: &mut Option<Position>) {
    if let (Some(index), Some(position)) = (chapter, current.take()) {
        boq.roots[index].positions.push(position);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_lv_keeps_quantities_and_distinguishes_duplicate_numbers() {
        let source = "Bauvorhaben: Test\nPos. / Leistungsbeschreibung Menge / Einheit\n1 Kapitel\n1.1 Erstes\n  65 m² ______ ______\n1.1 Zweites\n  18 St ______ ______\n";
        assert!(looks_like_compact_lv(source));
        let boq = parse_compact_lv("test.pdf", source).unwrap();
        assert_eq!(boq.roots.len(), 1);
        assert_eq!(boq.roots[0].positions.len(), 2);
        assert_eq!(boq.roots[0].positions[0].oz, "01.001");
        assert_eq!(boq.roots[0].positions[1].oz, "01.002");
        assert_eq!(boq.roots[0].positions[0].unit.as_deref(), Some("m²"));
        assert!(boq.warnings.iter().any(|w| w.contains("doppelter")));
    }
}
