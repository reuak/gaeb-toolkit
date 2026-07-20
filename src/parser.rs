use std::{collections::HashMap, path::Path, process::Command, str::FromStr};

use anyhow::{bail, Context, Result};
use regex::Regex;
use rust_decimal::Decimal;

use crate::model::{BillOfQuantities, Node, Position};

type HeadingMap = HashMap<String, (String, usize)>;

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
    let position_start_re =
        Regex::new(r"^(?P<oz>\d{2}\.\d{2}\.\d{2}\.\d{3})(?:\s+(?P<rest>.*))?$")?;
    let priced_data_re = Regex::new(
        r"^(?P<qty>[\d.]+,\d{3})\s+(?P<unit>\S+)\s+(?P<ep>[\d.]+,\d{2})\s*€(?:\s+(?P<gb>[\d.]+,\d{2})\s*€|\s+Nur\s+Einh\.-Pr\.)?\s*$",
    )?;
    let sum_re = Regex::new(r"^Summe\s+\d{2}(?:\.\d{2}){1,2}\b")?;
    let footer_re = Regex::new(r"^Druckausgabe vom:.*\d+\s*/\s*\d+\s*$")?;

    let mut boq = BillOfQuantities::new(source);
    let mut headings = HeadingMap::new();
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

            if let Some(caps) = position_start_re.captures(&line) {
                finish_position(
                    &mut boq,
                    &headings,
                    &mut current_position,
                    &mut position_lines,
                    page_number,
                );

                let rest = caps.name("rest").map(|v| v.as_str()).unwrap_or_default();
                let mut position = Position {
                    oz: caps["oz"].to_owned(),
                    page_from: Some(page_number),
                    ..Position::default()
                };

                if let Some(price_caps) = priced_data_re.captures(rest) {
                    position.quantity = parse_decimal(price_caps.name("qty").map(|v| v.as_str()));
                    position.unit = price_caps.name("unit").map(|v| v.as_str().to_owned());
                    position.unit_price = parse_decimal(price_caps.name("ep").map(|v| v.as_str()));
                    position.total_price = parse_decimal(price_caps.name("gb").map(|v| v.as_str()));
                    position.provisional = rest.contains("Nur Einh.-Pr.");
                    position.price_only = rest.contains("Nur Einh.-Pr.");
                } else if !rest.is_empty() {
                    position_lines.push(rest.to_owned());
                }

                current_position = Some(position);
                continue;
            }

            if let Some(caps) = heading_re.captures(&line) {
                finish_position(
                    &mut boq,
                    &headings,
                    &mut current_position,
                    &mut position_lines,
                    page_number,
                );
                headings.insert(
                    caps["oz"].to_owned(),
                    (
                        caps.name("title")
                            .map(|v| v.as_str().trim().to_owned())
                            .unwrap_or_default(),
                        page_number,
                    ),
                );
                continue;
            }

            if sum_re.is_match(&line) {
                finish_position(
                    &mut boq,
                    &headings,
                    &mut current_position,
                    &mut position_lines,
                    page_number,
                );
                continue;
            }

            if let Some(position) = current_position.as_mut() {
                if line == "Eventualposition ohne GB" {
                    position.provisional = true;
                    position.price_only = true;
                } else if line == "Position entfällt" {
                    position_lines.push(line);
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

    finish_position(
        &mut boq,
        &headings,
        &mut current_position,
        &mut position_lines,
        0,
    );
    boq.preamble = preamble_lines.join("\n").trim().to_owned();
    apply_heading_titles(&mut boq.roots, &headings);
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
    headings: &HeadingMap,
    current: &mut Option<Position>,
    lines: &mut Vec<String>,
    page_to: usize,
) {
    let Some(mut position) = current.take() else {
        return;
    };

    position.page_to = if page_to > 0 {
        Some(page_to)
    } else {
        position.page_from
    };

    let cleaned = lines
        .iter()
        .filter(|line| !line.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if let Some(first) = cleaned.first() {
        position.short_text = first.clone();
        position.long_text = cleaned.iter().skip(1).cloned().collect::<Vec<_>>().join("\n");
    }
    lines.clear();

    let hierarchy_oz = position
        .oz
        .split('.')
        .take(3)
        .collect::<Vec<_>>()
        .join(".");
    let parent = ensure_hierarchy_from_position(
        &mut boq.roots,
        &hierarchy_oz,
        headings,
        position.page_from.unwrap_or_default(),
    );
    parent.positions.push(position);
}

fn ensure_hierarchy_from_position<'a>(
    roots: &'a mut Vec<Node>,
    hierarchy_oz: &str,
    headings: &HeadingMap,
    fallback_page: usize,
) -> &'a mut Node {
    let parts = hierarchy_oz.split('.').collect::<Vec<_>>();
    ensure_path(roots, &parts, 0, headings, fallback_page)
}

fn ensure_path<'a>(
    nodes: &'a mut Vec<Node>,
    parts: &[&str],
    index: usize,
    headings: &HeadingMap,
    fallback_page: usize,
) -> &'a mut Node {
    let current_oz = parts[..=index].join(".");
    let heading = headings.get(&current_oz);
    let title = heading.map(|(title, _)| title.clone()).unwrap_or_default();
    let page = heading.map(|(_, page)| *page).unwrap_or(fallback_page);

    let node_index = match nodes.iter().position(|node| node.oz == current_oz) {
        Some(value) => value,
        None => {
            nodes.push(Node {
                oz: current_oz,
                title,
                level: index + 1,
                page: Some(page),
                children: Vec::new(),
                positions: Vec::new(),
            });
            nodes.len() - 1
        }
    };

    if nodes[node_index].title.is_empty() {
        if let Some((title, heading_page)) = heading {
            nodes[node_index].title = title.clone();
            nodes[node_index].page = Some(*heading_page);
        }
    }

    if index + 1 == parts.len() {
        return &mut nodes[node_index];
    }

    ensure_path(
        &mut nodes[node_index].children,
        parts,
        index + 1,
        headings,
        fallback_page,
    )
}

fn apply_heading_titles(nodes: &mut [Node], headings: &HeadingMap) {
    for node in nodes {
        if let Some((title, page)) = headings.get(&node.oz) {
            node.title = title.clone();
            node.page = Some(*page);
        }
        apply_heading_titles(&mut node.children, headings);
    }
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
        let omitted = position.short_text == "Position entfällt"
            || position.long_text.lines().any(|line| line == "Position entfällt");
        if !omitted && (position.quantity.is_none() || position.unit_price.is_none()) {
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
    fn builds_hierarchy_from_position_oz() {
        let text = "01 Logistik\n01.01 Vorbereitung\n01.01.02 Schutz\n01.01.02.030 361,000 qm 3,50 € 1.263,50 €\nBoden schützen\n";
        let boq = parse_text("test.txt", text).unwrap();
        assert_eq!(boq.roots.len(), 1);
        assert_eq!(boq.roots[0].oz, "01");
        assert_eq!(boq.roots[0].title, "Logistik");
        assert_eq!(boq.roots[0].children[0].oz, "01.01");
        assert_eq!(boq.roots[0].children[0].title, "Vorbereitung");
        assert_eq!(boq.roots[0].children[0].children[0].oz, "01.01.02");
        assert_eq!(boq.roots[0].children[0].children[0].title, "Schutz");
        assert_eq!(boq.roots[0].children[0].children[0].positions[0].oz, "01.01.02.030");
    }

    #[test]
    fn detects_position_without_prices() {
        let text = "01.01.01.120 Verkehrsrechtl. Beantragung Baustelleneinrichtung\nPosition entfällt\n";
        let boq = parse_text("test.txt", text).unwrap();
        let position = &boq.roots[0].children[0].children[0].positions[0];
        assert_eq!(position.oz, "01.01.01.120");
        assert_eq!(position.short_text, "Verkehrsrechtl. Beantragung Baustelleneinrichtung");
        assert_eq!(position.long_text, "Position entfällt");
        assert!(boq.warnings.is_empty());
    }

    #[test]
    fn ignores_unreferenced_headings() {
        let text = "01 Verwaister Bereich\n02.03.04.010 1,000 St 5,00 € 5,00 €\nPosition\n";
        let boq = parse_text("test.txt", text).unwrap();
        assert_eq!(boq.roots.len(), 1);
        assert_eq!(boq.roots[0].oz, "02");
        assert_eq!(boq.roots[0].children[0].oz, "02.03");
        assert_eq!(boq.roots[0].children[0].children[0].oz, "02.03.04");
    }
}
