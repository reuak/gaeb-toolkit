use std::{path::Path, process::Command, str::FromStr};

use anyhow::{bail, Context, Result};
use regex::Regex;
use rust_decimal::Decimal;

use crate::model::{BillOfQuantities, Node, Position};

pub fn parse_pdf(path: impl AsRef<Path>) -> Result<BillOfQuantities> {
    let path = path.as_ref();
    let output = Command::new("pdftotext")
        .args(["-layout", path.to_string_lossy().as_ref(), "-"])
        .output()
        .with_context(|| "pdftotext konnte nicht gestartet werden; bitte Poppler installieren")?;

    if !output.status.success() {
        bail!(
            "pdftotext ist fehlgeschlagen: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let text = String::from_utf8(output.stdout).context("PDF-Text ist nicht UTF-8")?;
    let source = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("input.pdf");
    parse_text(source, &text)
}

pub fn parse_text(source: &str, text: &str) -> Result<BillOfQuantities> {
    let heading_re = Regex::new(r"^(?P<oz>\d{2}(?:\.\d{2}){0,2})\s+(?P<title>\S.*)$")?;
    let position_re = Regex::new(
        r"^(?P<oz>\d{2}\.\d{2}\.\d{2}\.\d{3})\s+(?P<qty>[\d.]+,\d{3})\s+(?P<unit>\S+)\s+(?P<ep>[\d.]+,\d{2})\s*€(?:\s+(?P<gb>[\d.]+,\d{2})\s*€|\s+Nur\s+Einh\.-Pr\.)?\s*$",
    )?;
    let sum_re = Regex::new(r"^Summe\s+\d{2}(?:\.\d{2}){1,2}\b")?;
    let footer_re = Regex::new(r"^Druckausgabe vom:.*\d+\s*/\s*\d+\s*$")?;

    let mut boq = BillOfQuantities::new(source);
    let mut current_position: Option<Position> = None;
    let mut position_lines = Vec::<String>::new();
    let mut preamble_lines = Vec::<String>::new();

    for (page_index, page) in text.split('\u{000C}').enumerate() {
        let page_number = page_index + 1;
        extract_metadata(&mut boq, page);

        for raw in page.lines() {
            let line = normalize_line(raw);
            if is_noise(&line, &footer_re) {
                continue;
            }

            if let Some(caps) = position_re.captures(&line) {
                finish_position(&mut boq, &mut current_position, &mut position_lines, page_number);
                current_position = Some(Position {
                    oz: caps["oz"].to_owned(),
                    quantity: parse_decimal(caps.name("qty").map(|v| v.as_str())),
                    unit: caps.name("unit").map(|v| v.as_str().to_owned()),
                    unit_price: parse_decimal(caps.name("ep").map(|v| v.as_str())),
                    total_price: parse_decimal(caps.name("gb").map(|v| v.as_str())),
                    page_from: Some(page_number),
                    provisional: line.contains("Nur Einh.-Pr."),
                    price_only: line.contains("Nur Einh.-Pr."),
                    ..Position::default()
                });
                continue;
            }

            if let Some(caps) = heading_re.captures(&line) {
                finish_position(&mut boq, &mut current_position, &mut position_lines, page_number);
                let oz = caps["oz"].to_owned();
                ensure_hierarchy(
                    &mut boq.roots,
                    &oz,
                    caps.name("title").map(|v| v.as_str()).unwrap_or_default(),
                    page_number,
                );
                continue;
            }

            if sum_re.is_match(&line) {
                finish_position(&mut boq, &mut current_position, &mut position_lines, page_number);
                continue;
            }

            if let Some(position) = current_position.as_mut() {
                if line == "Eventualposition ohne GB" {
                    position.provisional = true;
                    position.price_only = true;
                } else if !matches!(
                    line.as_str(),
                    "Fortsetzung von vorheriger Seite" | "Fortsetzung auf nächster Seite"
                ) {
                    position_lines.push(line);
                }
            } else if page_number < 15 {
                preamble_lines.push(line);
            }
        }
    }

    finish_position(&mut boq, &mut current_position, &mut position_lines, 0);
    boq.preamble = preamble_lines.join("\n").trim().to_owned();
    validate(&mut boq);
    Ok(boq)
}

pub fn parse_decimal(value: Option<&str>) -> Option<Decimal> {
    let normalized = value?.replace('.', "").replace(',', ".");
    Decimal::from_str(&normalized).ok()
}

fn normalize_line(raw: &str) -> String {
    raw.replace('\u{00A0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_noise(line: &str, footer_re: &Regex) -> bool {
    const PREFIXES: &[&str] = &[
        "Angebot",
        "Auftraggeber ",
        "Bieter ",
        "Projekt ",
        "LV ",
        "OZ Menge / Einheit EP GB",
    ];
    line.is_empty() || footer_re.is_match(line) || PREFIXES.iter().any(|p| line.starts_with(p))
}

fn extract_metadata(boq: &mut BillOfQuantities, page: &str) {
    for raw in page.lines() {
        let line = normalize_line(raw);
        if boq.client.is_empty() {
            if let Some(value) = line.strip_prefix("Auftraggeber ") {
                boq.client = value.trim().to_owned();
            }
        }
        if boq.bidder.is_empty() {
            if let Some(value) = line.strip_prefix("Bieter ") {
                boq.bidder = value.trim().to_owned();
            }
        }
        if boq.project.is_empty() {
            if let Some(value) = line.strip_prefix("Projekt ") {
                boq.project = value.trim().to_owned();
            }
        }
    }
}

fn finish_position(
    boq: &mut BillOfQuantities,
    current: &mut Option<Position>,
    lines: &mut Vec<String>,
    page_to: usize,
) {
    let Some(mut position) = current.take() else {
        return;
    };
    if page_to > 0 {
        position.page_to = Some(page_to);
    } else {
        position.page_to = position.page_from;
    }
    if let Some(first) = lines.first() {
        position.short_text = first.clone();
        position.long_text = lines.iter().skip(1).cloned().collect::<Vec<_>>().join("\n");
    }
    lines.clear();

    let parent_oz = position
        .oz
        .split('.')
        .take(3)
        .collect::<Vec<_>>()
        .join(".");
    let parent = ensure_hierarchy(
        &mut boq.roots,
        &parent_oz,
        "",
        position.page_from.unwrap_or_default(),
    );
    parent.positions.push(position);
}

fn ensure_hierarchy<'a>(roots: &'a mut Vec<Node>, oz: &str, title: &str, page: usize) -> &'a mut Node {
    let parts = oz.split('.').collect::<Vec<_>>();
    ensure_path(roots, &parts, 0, title, page)
}

fn ensure_path<'a>(
    nodes: &'a mut Vec<Node>,
    parts: &[&str],
    index: usize,
    final_title: &str,
    page: usize,
) -> &'a mut Node {
    let current_oz = parts[..=index].join(".");
    let position = nodes.iter().position(|node| node.oz == current_oz);
    let node_index = match position {
        Some(value) => value,
        None => {
            nodes.push(Node {
                oz: current_oz,
                title: if index + 1 == parts.len() {
                    final_title.to_owned()
                } else {
                    String::new()
                },
                level: index + 1,
                page: Some(page),
                children: Vec::new(),
                positions: Vec::new(),
            });
            nodes.len() - 1
        }
    };

    if index + 1 == parts.len() {
        if nodes[node_index].title.is_empty() && !final_title.is_empty() {
            nodes[node_index].title = final_title.to_owned();
        }
        return &mut nodes[node_index];
    }

    ensure_path(
        &mut nodes[node_index].children,
        parts,
        index + 1,
        final_title,
        page,
    )
}

fn validate(boq: &mut BillOfQuantities) {
    let mut seen = std::collections::HashSet::new();
    let mut warnings = Vec::new();
    for root in &boq.roots {
        validate_node(root, &mut seen, &mut warnings);
    }
    boq.warnings.extend(warnings);
}

fn validate_node(
    node: &Node,
    seen: &mut std::collections::HashSet<String>,
    warnings: &mut Vec<String>,
) {
    for position in &node.positions {
        if !seen.insert(position.oz.clone()) {
            warnings.push(format!("Doppelte OZ: {}", position.oz));
        }
        if position.quantity.is_none() || position.unit_price.is_none() {
            warnings.push(format!("Unvollständige Preiszeile: {}", position.oz));
        }
        if let (Some(quantity), Some(unit_price), Some(total)) =
            (position.quantity, position.unit_price, position.total_price)
        {
            let difference = (quantity * unit_price - total).abs();
            if difference > Decimal::new(2, 2) {
                warnings.push(format!("Preisabweichung {}: {}", position.oz, difference));
            }
        }
    }
    for child in &node.children {
        validate_node(child, seen, warnings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_german_decimal() {
        assert_eq!(parse_decimal(Some("1.263,50")), Some(Decimal::new(126350, 2)));
        assert_eq!(parse_decimal(Some("361,000")), Some(Decimal::new(361000, 3)));
    }

    #[test]
    fn builds_oz_hierarchy() {
        let text = "01 Logistik\n01.01 Vorbereitung\n01.01.02 Schutz\n01.01.02.030 361,000 qm 3,50 € 1.263,50 €\nBoden schützen\n";
        let boq = parse_text("test.txt", text).unwrap();
        assert_eq!(boq.roots[0].oz, "01");
        assert_eq!(boq.roots[0].children[0].children[0].title, "Schutz");
        assert_eq!(boq.roots[0].children[0].children[0].positions[0].oz, "01.01.02.030");
    }
}
