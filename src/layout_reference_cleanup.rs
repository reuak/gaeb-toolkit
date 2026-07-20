use std::{collections::{HashMap, HashSet}, fs, path::Path, process::Command};

use anyhow::{Context, Result};
use quick_xml::escape::unescape;
use regex::Regex;
use tempfile::tempdir;

use crate::model::{BillOfQuantities, Node, Position};

#[derive(Debug, Clone)]
struct OzOccurrence {
    page: usize,
    top: i32,
    left: i32,
    oz: String,
}

/// Entfernt vollständige OZ, die im PDF eingerückt innerhalb eines
/// Positionstextes stehen. Echte Positions-OZ stehen am linken OZ-Rand;
/// eingerückte und unbepreiste Treffer werden an die vorherige echte Position
/// zurückgehängt.
pub fn repair_indented_references(path: &Path, boq: &mut BillOfQuantities) -> Result<usize> {
    let occurrences = extract_occurrences(path)?;
    if occurrences.is_empty() {
        return Ok(0);
    }

    let margin = occurrences.iter().map(|value| value.left).min().unwrap_or_default();
    let mut repaired = 0usize;

    for (index, occurrence) in occurrences.iter().enumerate() {
        // Poppler-Koordinaten schwanken geringfügig. Alles deutlich rechts vom
        // kleinsten OZ-Rand ist Fließtext und keine neue Position.
        if occurrence.left <= margin + 18 {
            continue;
        }

        let Some(candidate) = take_unpriced_position(
            &mut boq.roots,
            &occurrence.oz,
            occurrence.page,
        ) else {
            continue;
        };

        let target = occurrences[..index]
            .iter()
            .rev()
            .find(|previous| previous.left <= margin + 18)
            .cloned();

        if let Some(target) = target {
            if let Some(position) = find_position_mut(&mut boq.roots, &target.oz, target.page) {
                append_reference(position, &candidate);
                repaired += 1;
                continue;
            }
        }

        // Ohne sicheren Vorgänger nichts verlieren.
        insert_position(&mut boq.roots, candidate);
    }

    if repaired > 0 {
        refresh_warnings(boq);
    }
    Ok(repaired)
}

fn extract_occurrences(path: &Path) -> Result<Vec<OzOccurrence>> {
    let dir = tempdir()?;
    let xml_path = dir.path().join("layout.xml");
    let output = Command::new("pdftohtml")
        .args([
            "-xml",
            "-hidden",
            "-nodrm",
            "-enc",
            "UTF-8",
            path.to_string_lossy().as_ref(),
            xml_path.to_string_lossy().as_ref(),
        ])
        .output()
        .with_context(|| "pdftohtml konnte für die eingerückte OZ-Prüfung nicht gestartet werden")?;

    if !output.status.success() {
        anyhow::bail!(
            "pdftohtml ist fehlgeschlagen: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let layout = fs::read_to_string(&xml_path)
        .with_context(|| format!("PDF-Layout konnte nicht gelesen werden: {}", xml_path.display()))?;
    parse_occurrences(&layout)
}

fn parse_occurrences(layout: &str) -> Result<Vec<OzOccurrence>> {
    let page_re = Regex::new(r#"(?s)<page\b(?P<attrs>[^>]*)>(?P<body>.*?)</page>"#)?;
    let text_re = Regex::new(r#"(?s)<text\b(?P<attrs>[^>]*)>(?P<body>.*?)</text>"#)?;
    let tag_re = Regex::new(r#"<[^>]+>"#)?;
    let oz_re = Regex::new(r#"^(?P<oz>\d{2}\.\d{2}\.\d{2}\.\d{3})(?:\s|$)"#)?;
    let mut result = Vec::new();

    for page_caps in page_re.captures_iter(layout) {
        let page_attrs = page_caps.name("attrs").map(|v| v.as_str()).unwrap_or_default();
        let Some(page) = attr(page_attrs, "number").and_then(|v| v.parse().ok()) else {
            continue;
        };
        let body = page_caps.name("body").map(|v| v.as_str()).unwrap_or_default();

        for text_caps in text_re.captures_iter(body) {
            let attrs = text_caps.name("attrs").map(|v| v.as_str()).unwrap_or_default();
            let top = attr(attrs, "top").and_then(|v| v.parse().ok()).unwrap_or_default();
            let left = attr(attrs, "left").and_then(|v| v.parse().ok()).unwrap_or_default();
            let raw = text_caps.name("body").map(|v| v.as_str()).unwrap_or_default();
            let stripped = tag_re.replace_all(raw, "");
            let text = unescape(&stripped.replace("&nbsp;", " "))
                .map(|v| normalize(&v))
                .unwrap_or_else(|_| normalize(&stripped));
            let Some(caps) = oz_re.captures(&text) else { continue };
            result.push(OzOccurrence {
                page,
                top,
                left,
                oz: caps["oz"].to_owned(),
            });
        }
    }

    result.sort_by_key(|value| (value.page, value.top, value.left));
    Ok(result)
}

fn take_unpriced_position(nodes: &mut [Node], oz: &str, page: usize) -> Option<Position> {
    for node in nodes {
        if let Some(index) = node.positions.iter().position(|position| {
            position.oz == oz
                && position.page_from == Some(page)
                && position.quantity.is_none()
                && position.unit.as_deref().unwrap_or_default().trim().is_empty()
                && position.unit_price.is_none()
                && position.total_price.is_none()
        }) {
            return Some(node.positions.remove(index));
        }
        if let Some(found) = take_unpriced_position(&mut node.children, oz, page) {
            return Some(found);
        }
    }
    None
}

fn find_position_mut<'a>(nodes: &'a mut [Node], oz: &str, page: usize) -> Option<&'a mut Position> {
    for node in nodes {
        if let Some(position) = node.positions.iter_mut().find(|position| {
            position.oz == oz && position.page_from == Some(page)
        }) {
            return Some(position);
        }
        if let Some(found) = find_position_mut(&mut node.children, oz, page) {
            return Some(found);
        }
    }
    None
}

fn append_reference(target: &mut Position, candidate: &Position) {
    let text = [candidate.short_text.trim(), candidate.long_text.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let continuation = if text.is_empty() {
        candidate.oz.clone()
    } else {
        format!("{} {}", candidate.oz, text)
    };

    if target.long_text.trim().is_empty() {
        target.long_text = continuation;
    } else {
        target.long_text.push('\n');
        target.long_text.push_str(&continuation);
    }
    if let Some(page_to) = candidate.page_to.or(candidate.page_from) {
        target.page_to = Some(target.page_to.unwrap_or(page_to).max(page_to));
    }
}

fn insert_position(nodes: &mut [Node], position: Position) {
    let parent_oz = position.oz.split('.').take(3).collect::<Vec<_>>().join(".");
    if let Some(parent) = find_node_mut(nodes, &parent_oz) {
        parent.positions.push(position);
    }
}

fn find_node_mut<'a>(nodes: &'a mut [Node], oz: &str) -> Option<&'a mut Node> {
    for node in nodes {
        if node.oz == oz {
            return Some(node);
        }
        if let Some(found) = find_node_mut(&mut node.children, oz) {
            return Some(found);
        }
    }
    None
}

fn refresh_warnings(boq: &mut BillOfQuantities) {
    let mut counts = HashMap::<String, usize>::new();
    collect_counts(&boq.roots, &mut counts);
    let existing = counts.keys().cloned().collect::<HashSet<_>>();
    let duplicates = counts.iter()
        .filter(|(_, count)| **count > 1)
        .map(|(oz, _)| oz.clone())
        .collect::<HashSet<_>>();

    boq.warnings.retain(|warning| {
        if let Some(oz) = warning.strip_prefix("Doppelte OZ: ") {
            return duplicates.contains(oz.trim());
        }
        if let Some(oz) = warning.strip_prefix("Unvollständige Preiszeile: ") {
            return existing.contains(oz.trim());
        }
        true
    });
}

fn collect_counts(nodes: &[Node], counts: &mut HashMap<String, usize>) {
    for node in nodes {
        for position in &node.positions {
            *counts.entry(position.oz.clone()).or_default() += 1;
        }
        collect_counts(&node.children, counts);
    }
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let double = format!("{name}=\"");
    if let Some(start) = attrs.find(&double) {
        let rest = &attrs[start + double.len()..];
        return rest.find('"').map(|end| rest[..end].to_owned());
    }
    let single = format!("{name}='");
    let start = attrs.find(&single)?;
    let rest = &attrs[start + single.len()..];
    rest.find('\'').map(|end| rest[..end].to_owned())
}

fn normalize(value: &str) -> String {
    value.replace('\u{00A0}', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[test]
    fn detects_indented_reference_after_real_position() {
        let xml = r#"<pdf2xml><page number="1">
          <text top="100" left="40">02.13.01.020 1,000 St 5,00 € 5,00 €</text>
          <text top="180" left="180">02.02.01.010</text>
        </page></pdf2xml>"#;
        let found = parse_occurrences(xml).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found[1].left > found[0].left + 18);
    }

    #[test]
    fn removes_only_unpriced_indented_candidate() {
        let mut nodes = vec![Node {
            oz: "02.13.01".into(),
            positions: vec![Position {
                oz: "02.13.01.020".into(),
                quantity: Some(Decimal::ONE),
                unit: Some("St".into()),
                unit_price: Some(Decimal::ONE),
                page_from: Some(1),
                ..Position::default()
            }],
            ..Node::default()
        }, Node {
            oz: "02.02.01".into(),
            positions: vec![Position {
                oz: "02.02.01.010".into(),
                short_text: "beschrieben".into(),
                page_from: Some(1),
                ..Position::default()
            }],
            ..Node::default()
        }];
        let candidate = take_unpriced_position(&mut nodes, "02.02.01.010", 1).unwrap();
        let target = find_position_mut(&mut nodes, "02.13.01.020", 1).unwrap();
        append_reference(target, &candidate);
        assert!(target.long_text.contains("02.02.01.010 beschrieben"));
    }
}
