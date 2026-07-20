use std::{collections::HashMap, path::Path, process::Command, str::FromStr};

use anyhow::{bail, Context, Result};
use regex::Regex;
use rust_decimal::Decimal;

use crate::model::{BillOfQuantities, Node, Position};

/// Liest Positionen wie `02.06.__.010` nach, die der normale numerische
/// OZ-Parser nicht erkennt. Der nichtnumerische Platzhalter `__` wird für den
/// GAEB-Export als `00` normalisiert, damit NOVA AVA die OZ importieren kann.
pub fn recover_placeholder_positions(path: &Path, boq: &mut BillOfQuantities) -> Result<usize> {
    let output = Command::new("pdftotext")
        .args(["-layout", path.to_string_lossy().as_ref(), "-"])
        .output()
        .with_context(|| "pdftotext konnte für Platzhalter-OZ nicht gestartet werden")?;

    if !output.status.success() {
        bail!(
            "pdftotext für Platzhalter-OZ ist fehlgeschlagen: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let text = String::from_utf8(output.stdout).context("PDF-Text ist nicht UTF-8")?;
    recover_placeholder_positions_from_text(&text, boq)
}

/// Nutzt bereits extrahierten `pdftotext -layout`-Text, damit dieselbe PDF im
/// normalen Parse-Ablauf nicht ein zweites Mal von Poppler gelesen werden muss.
pub(crate) fn recover_placeholder_positions_from_text(
    text: &str,
    boq: &mut BillOfQuantities,
) -> Result<usize> {
    recover_from_text(text, boq)
}

fn recover_from_text(text: &str, boq: &mut BillOfQuantities) -> Result<usize> {
    let heading_re = Regex::new(r"^(?P<oz>\d{2}\.\d{2})\s+(?P<title>\S.*)$")?;
    let position_re = Regex::new(
        r"^(?P<a>\d{2})\.(?P<b>\d{2})\.__\.(?P<item>\d{3})(?:\s+(?P<rest>.*))?$",
    )?;
    let any_position_re = Regex::new(r"^\d{2}\.\d{2}(?:\.(?:\d{2}|__)){1,2}(?:\s|$)")?;
    let sum_re = Regex::new(r"^Summe\s+\d{2}\.\d{2}\b")?;
    let price_re = Regex::new(
        r"^(?P<qty>[\d.]+,\d{3})\s+(?P<unit>\S+)\s+(?P<ep>[\d.]+,\d{2})\s*€(?:\s+(?P<gb>[\d.]+,\d{2})\s*€|\s+Nur\s+Einh\.-Pr\.)?\s*$",
    )?;

    let mut headings = HashMap::<String, (String, usize)>::new();
    let mut recovered = Vec::<Position>::new();
    let mut current: Option<Position> = None;
    let mut lines = Vec::<String>::new();

    for (page_index, page) in text.split('\u{000C}').enumerate() {
        let page_number = page_index + 1;
        for raw in page.lines() {
            let line = normalize(raw);
            if line.is_empty() || is_noise(&line) {
                continue;
            }

            if let Some(caps) = position_re.captures(&line) {
                finish_current(&mut current, &mut lines, page_number, &mut recovered, &price_re);
                let normalized_oz = format!("{}.{}.00.{}", &caps["a"], &caps["b"], &caps["item"]);
                let mut position = Position {
                    oz: normalized_oz,
                    page_from: Some(page_number),
                    ..Position::default()
                };
                let rest = caps.name("rest").map(|v| v.as_str()).unwrap_or_default();
                if let Some(price_caps) = price_re.captures(rest) {
                    apply_price(&mut position, &price_caps, rest);
                } else if !rest.is_empty() {
                    lines.push(rest.to_owned());
                }
                current = Some(position);
                continue;
            }

            if let Some(caps) = heading_re.captures(&line) {
                if caps["oz"].ends_with(".06") || current.is_none() {
                    headings.insert(
                        caps["oz"].to_owned(),
                        (caps["title"].trim().to_owned(), page_number),
                    );
                }
            }

            if current.is_some() && (sum_re.is_match(&line) || any_position_re.is_match(&line)) {
                finish_current(&mut current, &mut lines, page_number, &mut recovered, &price_re);
                continue;
            }

            if let Some(position) = current.as_mut() {
                if line == "Eventualposition ohne GB" {
                    position.provisional = true;
                    position.price_only = true;
                } else if !matches!(
                    line.as_str(),
                    "Fortsetzung von vorheriger Seite" | "Fortsetzung auf nächster Seite"
                ) {
                    lines.push(line);
                }
            }
        }
    }

    let last_page = text.split('\u{000C}').count().max(1);
    finish_current(&mut current, &mut lines, last_page, &mut recovered, &price_re);

    let mut count = 0usize;
    for position in recovered {
        if contains_oz(&boq.roots, &position.oz) {
            continue;
        }
        insert_position(boq, position, &headings);
        count += 1;
    }

    sort_hierarchy(&mut boq.roots);
    Ok(count)
}

fn finish_current(
    current: &mut Option<Position>,
    lines: &mut Vec<String>,
    page_to: usize,
    recovered: &mut Vec<Position>,
    price_re: &Regex,
) {
    let Some(mut position) = current.take() else {
        return;
    };
    position.page_to = Some(page_to.max(position.page_from.unwrap_or(page_to)));

    if position.quantity.is_none() {
        let max = lines.len().min(4);
        for count in (1..=max).rev() {
            let candidate = lines[..count].join(" ");
            if let Some(caps) = price_re.captures(&candidate) {
                apply_price(&mut position, &caps, &candidate);
                lines.drain(..count);
                break;
            }
        }
    }

    let clean = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if let Some(first) = clean.first() {
        position.short_text = first.clone();
        position.long_text = clean.iter().skip(1).cloned().collect::<Vec<_>>().join("\n");
    }
    lines.clear();

    if position.quantity.is_some()
        || position.unit_price.is_some()
        || !position.short_text.trim().is_empty()
        || !position.long_text.trim().is_empty()
    {
        recovered.push(position);
    }
}

fn apply_price(position: &mut Position, caps: &regex::Captures<'_>, source: &str) {
    position.quantity = parse_decimal(caps.name("qty").map(|v| v.as_str()));
    position.unit = caps.name("unit").map(|v| v.as_str().to_owned());
    position.unit_price = parse_decimal(caps.name("ep").map(|v| v.as_str()));
    position.total_price = parse_decimal(caps.name("gb").map(|v| v.as_str()));
    position.provisional |= source.contains("Nur Einh.-Pr.");
    position.price_only |= source.contains("Nur Einh.-Pr.");
}

fn parse_decimal(value: Option<&str>) -> Option<Decimal> {
    let normalized = value?.replace('.', "").replace(',', ".");
    Decimal::from_str(&normalized).ok()
}

fn insert_position(
    boq: &mut BillOfQuantities,
    position: Position,
    headings: &HashMap<String, (String, usize)>,
) {
    let parts = position.oz.split('.').take(3).collect::<Vec<_>>();
    let parent = ensure_path(
        &mut boq.roots,
        &parts,
        0,
        headings,
        position.page_from.unwrap_or_default(),
    );
    parent.positions.push(position);
}

fn ensure_path<'a>(
    nodes: &'a mut Vec<Node>,
    parts: &[&str],
    index: usize,
    headings: &HashMap<String, (String, usize)>,
    fallback_page: usize,
) -> &'a mut Node {
    let oz = parts[..=index].join(".");
    let heading = headings.get(&oz);
    let node_index = nodes.iter().position(|node| node.oz == oz).unwrap_or_else(|| {
        nodes.push(Node {
            oz: oz.clone(),
            title: heading.map(|(title, _)| title.clone()).unwrap_or_default(),
            level: index + 1,
            page: Some(heading.map(|(_, page)| *page).unwrap_or(fallback_page)),
            children: Vec::new(),
            positions: Vec::new(),
        });
        nodes.len() - 1
    });

    if nodes[node_index].title.is_empty() {
        if let Some((title, page)) = heading {
            nodes[node_index].title = title.clone();
            nodes[node_index].page = Some(*page);
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

fn sort_hierarchy(nodes: &mut Vec<Node>) {
    nodes.sort_by(|left, right| oz_sort_key(&left.oz).cmp(&oz_sort_key(&right.oz)));
    for node in nodes {
        sort_hierarchy(&mut node.children);
        node.positions
            .sort_by(|left, right| oz_sort_key(&left.oz).cmp(&oz_sort_key(&right.oz)));
    }
}

fn oz_sort_key(oz: &str) -> Vec<u32> {
    oz.split('.')
        .map(|part| part.parse::<u32>().unwrap_or(u32::MAX))
        .collect()
}

fn contains_oz(nodes: &[Node], oz: &str) -> bool {
    nodes.iter().any(|node| {
        node.positions.iter().any(|position| position.oz == oz) || contains_oz(&node.children, oz)
    })
}

fn normalize(raw: &str) -> String {
    raw.replace('\u{00A0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_noise(line: &str) -> bool {
    line.starts_with("OZ Menge / Einheit")
        || line.starts_with("Druckausgabe vom:")
        || line.starts_with("Angebot")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_placeholder_positions_and_normalizes_oz() {
        let text = "02 Ausbauarbeiten\n02.06 Tapezierarbeiten\n02.06.__.010 1.621,000 qm 2,80 € 4.538,80 €\nGrundieren Wandflächen Tapetengrund\nBeschreibung\n02.06.__.020 1.627,000 qm 14,50 € 23.591,50 €\nMalervlies Wandflächen Neu\nSumme 02.06 Tapezierarbeiten 28.130,30 €\n";
        let mut boq = BillOfQuantities::new("test.pdf");
        let count = recover_from_text(text, &mut boq).unwrap();
        assert_eq!(count, 2);
        let title = &boq.roots[0].children[0];
        assert_eq!(title.oz, "02.06");
        assert_eq!(title.title, "Tapezierarbeiten");
        let placeholder = &title.children[0];
        assert_eq!(placeholder.oz, "02.06.00");
        assert_eq!(placeholder.positions[0].oz, "02.06.00.010");
        assert_eq!(placeholder.positions[1].oz, "02.06.00.020");
    }

    #[test]
    fn sorts_placeholder_level_before_numbered_levels() {
        let mut nodes = vec![Node {
            oz: "02".into(),
            children: vec![Node {
                oz: "02.04".into(),
                children: vec![
                    Node { oz: "02.04.01".into(), ..Node::default() },
                    Node { oz: "02.04.00".into(), ..Node::default() },
                    Node { oz: "02.04.02".into(), ..Node::default() },
                ],
                ..Node::default()
            }],
            ..Node::default()
        }];

        sort_hierarchy(&mut nodes);
        let children = &nodes[0].children[0].children;
        assert_eq!(children[0].oz, "02.04.00");
        assert_eq!(children[1].oz, "02.04.01");
        assert_eq!(children[2].oz, "02.04.02");
    }
}
