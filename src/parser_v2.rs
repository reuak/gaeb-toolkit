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
    let heading_re = Regex::new(r"^(?P<oz>\d+(?:\.\d+){0,4})(?P<trailing>\.)?\s+(?P<title>\S.*)$")?;
    let position_start_re =
        Regex::new(r"^(?P<oz>\d+(?:\.\d+){2,5})(?P<trailing>\.)?(?:\s+(?P<rest>.*))?$")?;
    let six_part_profile_re = Regex::new(r"(?m)^\s*\d+(?:\.\d+){5}\s+\S")?;
    let six_part_profile = six_part_profile_re.is_match(text);
    let priced_data_re = priced_data_regex()?;
    let sum_re = Regex::new(r"^Summe\s+\d+(?:\.\d+){1,5}\.?(?:\s|$)")?;
    let footer_re = Regex::new(r"^Druckausgabe vom:.*\d+\s*/\s*\d+\s*$")?;

    let mut boq = BillOfQuantities::new(source);
    let mut headings = HeadingMap::new();
    let mut current_position: Option<Position> = None;
    let mut position_lines = Vec::<String>::new();
    let mut preamble_lines = Vec::<String>::new();
    let mut last_content_page = 1usize;

    for (page_index, page) in text.split('\u{000C}').enumerate() {
        let page_number = page_index + 1;
        if !page.trim().is_empty() {
            last_content_page = page_number;
        }
        extract_metadata(&mut boq, page);

        for raw in page.lines() {
            let line = normalize_line(raw);
            if is_noise(&line, &footer_re) {
                continue;
            }

            if let Some(caps) = position_start_re
                .captures(&line)
                .filter(|caps| is_position_oz(caps, six_part_profile))
            {
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
                    apply_price_captures(&mut position, &price_caps, rest);
                } else if !rest.is_empty() {
                    position_lines.push(rest.to_owned());
                }

                current_position = Some(position);
                continue;
            }

            if let Some(caps) = heading_re
                .captures(&line)
                .filter(|caps| is_heading_oz(caps, current_position.is_some()))
            {
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

            if sum_re.is_match(&line)
                || line.starts_with("Titelsumme:")
                || line.starts_with("Gewerksumme:")
            {
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
        last_content_page,
    );
    boq.preamble = preamble_lines.join("\n").trim().to_owned();
    apply_heading_titles(&mut boq.roots, &headings);
    validate(&mut boq);
    Ok(boq)
}

fn priced_data_regex() -> Result<Regex, regex::Error> {
    Regex::new(
        r"^(?P<qty>[\d.]+,\d{3})\s+(?P<unit>\S+)\s+(?P<ep>[\d.]+,\d{2})\s*€(?:\s+(?P<gb>[\d.]+,\d{2})\s*€|\s+Nur\s+Einh\.-Pr\.)?\s*$",
    )
}

fn trailing_quantity_regex() -> Result<Regex, regex::Error> {
    Regex::new(
        r"(?P<qty>\d[\d.]*(?:,\d{1,3})?)\s+(?P<unit>\S+)\s+(?:(?P<ep>-?[\d.]+,\d{2})\.?|\.{3,})\s*(?:€|EUR)?(?:\s+(?:Bedarf\s+)?(?:(?P<gb>-?[\d.]+,\d{2})|\.{3,})\s*(?:€|EUR)?|\s+(?P<price_only>(?:Nur\s+Einh\.-Pr\.|nur\s+EP)))?\s*$",
    )
}

fn is_position_oz(caps: &regex::Captures<'_>, six_part_profile: bool) -> bool {
    let components = caps["oz"].split('.').collect::<Vec<_>>();
    if is_date_like_oz(&components) {
        return false;
    }
    if six_part_profile {
        return components.len() == 6;
    }
    match components.len() {
        3 => caps.name("trailing").is_some() || components[2].len() >= 3,
        4 => components[3].len() >= 3,
        6 => true,
        _ => false,
    }
}

fn is_date_like_oz(components: &[&str]) -> bool {
    if components.len() != 3 || components[2].len() != 4 {
        return false;
    }
    let values = components
        .iter()
        .map(|component| component.parse::<u32>())
        .collect::<Result<Vec<_>, _>>();
    values.is_ok_and(|values| {
        (1..=31).contains(&values[0])
            && (1..=12).contains(&values[1])
            && (1900..=2100).contains(&values[2])
    })
}

fn is_heading_oz(caps: &regex::Captures<'_>, position_active: bool) -> bool {
    let components = caps["oz"].split('.').collect::<Vec<_>>();
    if caps.name("trailing").is_some() {
        components.iter().all(|component| component.len() <= 2)
            && !(position_active && components.len() == 1)
    } else {
        components.len() <= 3
            && components.iter().all(|component| component.len() == 2)
            && !(position_active && components.len() == 1)
    }
}

fn apply_price_captures(position: &mut Position, caps: &regex::Captures<'_>, source: &str) {
    position.quantity = parse_decimal(caps.name("qty").map(|v| v.as_str()));
    position.unit = caps.name("unit").map(|v| v.as_str().to_owned());
    position.unit_price = parse_decimal(caps.name("ep").map(|v| v.as_str()));
    position.total_price = parse_decimal(caps.name("gb").map(|v| v.as_str()));
    position.provisional |= source.contains("Nur Einh.-Pr.");
    position.price_only |= source.contains("Nur Einh.-Pr.");
}

fn extract_leading_multiline_price(position: &mut Position, lines: &mut Vec<String>) {
    if position.quantity.is_some() || lines.is_empty() {
        return;
    }

    let Ok(price_re) = priced_data_regex() else {
        return;
    };
    let max_fragments = lines.len().min(4);

    // Längste passende Variante zuerst prüfen. Dadurch wird bei getrennten
    // Zeilen der Gesamtbetrag nicht als Beginn des Kurztexts stehen gelassen.
    for count in (1..=max_fragments).rev() {
        let candidate = lines[..count].join(" ");
        if let Some(caps) = price_re.captures(&candidate) {
            apply_price_captures(position, &caps, &candidate);
            lines.drain(..count);
            return;
        }
    }
}

fn extract_trailing_quantity(position: &mut Position, lines: &mut Vec<String>) {
    if position.quantity.is_some() {
        return;
    }

    let Ok(quantity_re) = trailing_quantity_regex() else {
        return;
    };
    let Some((index, caps)) = lines
        .iter()
        .enumerate()
        .find_map(|(index, line)| quantity_re.captures(line).map(|caps| (index, caps)))
    else {
        return;
    };

    position.quantity = parse_decimal(caps.name("qty").map(|value| value.as_str()));
    position.unit = caps.name("unit").map(|value| value.as_str().to_owned());
    position.unit_price = parse_decimal(caps.name("ep").map(|value| value.as_str()));
    position.total_price = parse_decimal(caps.name("gb").map(|value| value.as_str()));
    position.provisional |= caps.name("price_only").is_some();
    position.price_only |= caps.name("price_only").is_some();
    let prefix = caps
        .get(0)
        .and_then(|matched| lines[index].get(..matched.start()))
        .unwrap_or_default()
        .trim()
        .to_owned();
    if prefix.is_empty() {
        lines.remove(index);
    } else {
        lines[index] = prefix;
    }
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
        "Angebotsaufforderung_",
        "Auftraggeber ",
        "Bieter ",
        "Druckdatum:",
        "Projekt ",
        "Projekt:",
        "LV ",
        "LV:",
        "Mollmann Beratende Ingenieure GmbH",
        "T. +49 ",
        "Übertrag:",
        "OZ Menge / Einheit EP GB",
        "OZ Leistungsbeschreibung",
    ];
    line.is_empty()
        || line == "in EUR in EUR"
        || footer_re.is_match(line)
        || PREFIXES.iter().any(|p| line.starts_with(p))
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

    position.page_to = Some(page_to.max(position.page_from.unwrap_or(page_to)));
    extract_leading_multiline_price(&mut position, lines);
    extract_trailing_quantity(&mut position, lines);

    let cleaned = lines
        .iter()
        .filter(|line| !line.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if let Some(first) = cleaned.first() {
        position.short_text = first.clone();
        position.long_text = cleaned
            .iter()
            .skip(1)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
    }
    lines.clear();

    let mut hierarchy_parts = position.oz.split('.').collect::<Vec<_>>();
    hierarchy_parts.pop();
    let hierarchy_oz = hierarchy_parts.join(".");
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
            || position
                .long_text
                .lines()
                .any(|line| line == "Position entfällt");
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

    fn first_position(boq: &BillOfQuantities) -> &Position {
        &boq.roots[0].children[0].children[0].positions[0]
    }

    #[test]
    fn parses_german_decimal() {
        assert_eq!(
            parse_decimal(Some("1.263,50")),
            Some(Decimal::new(126350, 2))
        );
        assert_eq!(
            parse_decimal(Some("361,000")),
            Some(Decimal::new(361000, 3))
        );
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
        assert_eq!(first_position(&boq).oz, "01.01.02.030");
    }

    #[test]
    fn parses_tga_position_with_short_oz_and_trailing_dot() {
        assert!(trailing_quantity_regex()
            .unwrap()
            .is_match("1.000,000 m ......................... ........................."));
        let text = "1. Los Elektroinstallation\n1.1. Titel Kabel und Leitungen\n1.1.10. NYM-J 3x1,5 mm²\nKunststoff-Mantelleitung liefern und verlegen\n10 Rundleiter 16 mm² Cu\n3 Stromkreise anschließen\n7235. Gehäuse mit wirksamer Auskleidung\n1.000,000 m ......................... .........................\n";
        let boq = parse_text("test.txt", text).unwrap();

        assert_eq!(boq.roots.len(), 1);
        assert_eq!(boq.roots[0].oz, "1");
        assert_eq!(boq.roots[0].title, "Los Elektroinstallation");
        assert_eq!(boq.roots[0].children[0].oz, "1.1");
        assert_eq!(boq.roots[0].children[0].title, "Titel Kabel und Leitungen");
        let position = &boq.roots[0].children[0].positions[0];
        assert_eq!(position.oz, "1.1.10");
        assert_eq!(position.quantity, Some(Decimal::new(1000000, 3)));
        assert_eq!(position.unit.as_deref(), Some("m"));
        assert_eq!(position.unit_price, None);
        assert_eq!(position.total_price, None);
        assert_eq!(position.short_text, "NYM-J 3x1,5 mm²");
        assert_eq!(
            position.long_text,
            "Kunststoff-Mantelleitung liefern und verlegen\n10 Rundleiter 16 mm² Cu\n3 Stromkreise anschließen\n7235. Gehäuse mit wirksamer Auskleidung"
        );
    }

    #[test]
    fn does_not_treat_numbered_preamble_as_short_position_oz() {
        let text = "1.1 Grundlage der Ausschreibung\n1.2 Vertragsbedingungen\n2.3.2.1 Unterabschnitt\n28.10.2025 16:20 Uhr\n";
        let boq = parse_text("test.txt", text).unwrap();
        assert!(boq.roots.is_empty());
    }

    #[test]
    fn parses_tga_price_only_quantity_row() {
        let text = "1.1.20. Bedarfsposition\n1,000 Stck ......................... Nur Einh.-Pr.\n";
        let boq = parse_text("test.txt", text).unwrap();
        let position = &boq.roots[0].children[0].positions[0];

        assert_eq!(position.quantity, Some(Decimal::new(1000, 3)));
        assert_eq!(position.unit.as_deref(), Some("Stck"));
        assert!(position.provisional);
        assert!(position.price_only);
    }

    #[test]
    fn parses_three_part_oz_without_trailing_dot() {
        let text =
            "02.01 Bereich\n02.01.0001 Randdämmstreifen abschneiden\n520,000 m² 9,40 4.888,00\n";
        let boq = parse_text("test.txt", text).unwrap();
        let position = &boq.roots[0].children[0].positions[0];

        assert_eq!(position.oz, "02.01.0001");
        assert_eq!(position.quantity, Some(Decimal::new(520000, 3)));
        assert_eq!(position.unit.as_deref(), Some("m²"));
        assert_eq!(position.unit_price, Some(Decimal::new(940, 2)));
        assert_eq!(position.total_price, Some(Decimal::new(488800, 2)));
    }

    #[test]
    fn parses_six_part_oz_and_inline_quantity() {
        let text = "6.35.10.1.1. Trockenbau\n6.35.10.1.1.10 GK-Doppelständerwand\nEinbauort: Obergeschoss 398,000 m2 71,84 28.592,32\n";
        let boq = parse_text("test.txt", text).unwrap();
        let position = &boq.roots[0].children[0].children[0].children[0].children[0].positions[0];

        assert_eq!(position.oz, "6.35.10.1.1.10");
        assert_eq!(position.quantity, Some(Decimal::new(398000, 3)));
        assert_eq!(position.unit.as_deref(), Some("m2"));
        assert_eq!(position.unit_price, Some(Decimal::new(7184, 2)));
        assert_eq!(position.total_price, Some(Decimal::new(2859232, 2)));
        assert_eq!(position.short_text, "GK-Doppelständerwand");
        assert_eq!(position.long_text, "Einbauort: Obergeschoss");
    }

    #[test]
    fn parses_integer_quantity_with_empty_price_columns() {
        let text = "01.01.01.0010 Einrichten Baustelle\nBeschreibung\n1. Brandschutz beachten\n1 St ...................... nur EP\n";
        let boq = parse_text("test.txt", text).unwrap();
        let position = first_position(&boq);

        assert_eq!(position.quantity, Some(Decimal::ONE));
        assert_eq!(position.unit.as_deref(), Some("St"));
        assert!(position.price_only);
        assert!(position.long_text.contains("1. Brandschutz beachten"));
    }

    #[test]
    fn parses_negative_and_required_prices() {
        let text = "01.03.0020 Minderpreis\nGKBI-Platte 3.794,000 m2 -1,50 -5.691,00\n01.03.0030 Bedarfsposition\nBeschreibung 1,000 m2 10,60 Bedarf 10,60\n";
        let boq = parse_text("test.txt", text).unwrap();
        let positions = &boq.roots[0].children[0].positions;

        assert_eq!(positions[0].unit_price, Some(Decimal::new(-150, 2)));
        assert_eq!(positions[0].total_price, Some(Decimal::new(-569100, 2)));
        assert_eq!(positions[1].unit_price, Some(Decimal::new(1060, 2)));
        assert_eq!(positions[1].total_price, Some(Decimal::new(1060, 2)));
    }

    #[test]
    fn removes_page_frames_and_subtotals_from_positions() {
        let text = "1.1.10. Leistung\nBeschreibung\n1,000 St ......................... .........................\nDruckdatum: 04.06.2026 Seite: 19 von 276\nMollmann Beratende Ingenieure GmbH Poststraße 13\nT. +49 6151 39728\nAngebotsaufforderung_MBI\nProjekt: 26004 Beispiel\nLV: 10 TGA\nOZ Leistungsbeschreibung Menge ME Einheitspreis Gesamtbetrag\nin EUR in EUR\nSumme 1.1. Titel .................\n1.2.10. Nächste Leistung\n2,000 St ......................... .........................\n";
        let boq = parse_text("test.txt", text).unwrap();
        let first = &boq.roots[0].children[0].positions[0];

        assert_eq!(first.long_text, "Beschreibung");
        assert!(!first.long_text.contains("Summe"));
        assert!(!first.long_text.contains("Druckdatum"));
        assert_eq!(boq.roots[0].children[1].positions.len(), 1);
    }

    #[test]
    fn detects_position_without_prices() {
        let text =
            "01.01.01.120 Verkehrsrechtl. Beantragung Baustelleneinrichtung\nPosition entfällt\n";
        let boq = parse_text("test.txt", text).unwrap();
        let position = first_position(&boq);
        assert_eq!(position.oz, "01.01.01.120");
        assert_eq!(
            position.short_text,
            "Verkehrsrechtl. Beantragung Baustelleneinrichtung"
        );
        assert_eq!(position.long_text, "Position entfällt");
        assert!(boq.warnings.is_empty());
    }

    #[test]
    fn parses_multiline_price_data() {
        let text = "01.01.01.020\n80,000 lfm\n38,70 €\n3.096,00 €\nBauzaun, Stahlrahmen mobil\nBeschreibung\n";
        let boq = parse_text("test.txt", text).unwrap();
        let position = first_position(&boq);
        assert_eq!(position.quantity, Some(Decimal::new(80000, 3)));
        assert_eq!(position.unit.as_deref(), Some("lfm"));
        assert_eq!(position.unit_price, Some(Decimal::new(3870, 2)));
        assert_eq!(position.total_price, Some(Decimal::new(309600, 2)));
        assert_eq!(position.short_text, "Bauzaun, Stahlrahmen mobil");
        assert_eq!(position.long_text, "Beschreibung");
    }

    #[test]
    fn keeps_position_across_page_break() {
        let text = "01.01.01.070 10,000 StMt 25,00 € 250,00 €\nBauzaun-Tor\nFortsetzung auf nächster Seite\n\u{000C}Fortsetzung von vorheriger Seite\nvorhalten und unterhalten\n";
        let boq = parse_text("test.txt", text).unwrap();
        let position = first_position(&boq);
        assert_eq!(position.page_from, Some(1));
        assert_eq!(position.page_to, Some(2));
        assert_eq!(position.short_text, "Bauzaun-Tor");
        assert_eq!(position.long_text, "vorhalten und unterhalten");
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
