use std::{collections::{HashMap, HashSet}, fs, path::Path, process::Command};

use anyhow::{Context, Result};
use quick_xml::escape::unescape;
use regex::Regex;
use tempfile::tempdir;

use crate::model::{BillOfQuantities, Node, Position};

#[derive(Debug)]
struct TextFragment {
    top: i32,
    left: i32,
    bold: bool,
    text: String,
}

#[derive(Debug)]
struct LayoutLine {
    top: i32,
    bold: bool,
    text: String,
}

/// Korrigiert PDF-spezifische Layoutartefakte nach dem textbasierten Parsing.
///
/// - mehrzeilig fett gesetzte Positionskurztexte bleiben vollständig erhalten
/// - reine Fließtext-Verweise wie „Zulage zu Position …“ und
///   „Positionsbezug …“ werden nicht als neue Positionen übernommen
pub fn postprocess_pdf(path: &Path, boq: &mut BillOfQuantities) -> Result<()> {
    remove_reference_positions(&mut boq.roots);

    match extract_bold_short_texts(path) {
        Ok(short_texts) => apply_bold_short_texts(&mut boq.roots, &short_texts),
        Err(error) => boq.warnings.push(format!(
            "Kurztext-Layout konnte nicht ausgewertet werden: {error}"
        )),
    }

    // Warnungen wurden vom Rohparser vor der Bereinigung erzeugt. Nur die
    // durch entfernte Fließtext-Verweise entstandenen Doppel-OZ-Warnungen
    // verwerfen; echte Konflikte bleiben bestehen.
    let duplicates = duplicate_ozs(&boq.roots);
    boq.warnings.retain(|warning| {
        warning
            .strip_prefix("Doppelte OZ: ")
            .map(|oz| duplicates.contains(oz))
            .unwrap_or(true)
    });
    Ok(())
}

fn remove_reference_positions(nodes: &mut [Node]) {
    for node in nodes {
        node.positions.retain(|position| !is_reference_position(position));
        remove_reference_positions(&mut node.children);
    }
}

fn is_reference_position(position: &Position) -> bool {
    let first = position
        .short_text
        .lines()
        .chain(position.long_text.lines())
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    is_reference_text(first)
}

fn is_reference_text(value: &str) -> bool {
    let lower = value.trim().to_lowercase();
    [
        "zulage zu position",
        "zulage zur position",
        "zulage zu pos.",
        "zulage zur pos.",
        "positionsbezug",
        "positions-bezug",
        "bezug zu position",
        "bezug auf position",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn duplicate_ozs(nodes: &[Node]) -> HashSet<String> {
    fn visit(nodes: &[Node], seen: &mut HashSet<String>, duplicates: &mut HashSet<String>) {
        for node in nodes {
            for position in &node.positions {
                if !seen.insert(position.oz.clone()) {
                    duplicates.insert(position.oz.clone());
                }
            }
            visit(&node.children, seen, duplicates);
        }
    }

    let mut seen = HashSet::new();
    let mut duplicates = HashSet::new();
    visit(nodes, &mut seen, &mut duplicates);
    duplicates
}

fn extract_bold_short_texts(path: &Path) -> Result<HashMap<(String, usize), String>> {
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
        .with_context(|| "pdftohtml konnte für die Kurztext-Erkennung nicht gestartet werden")?;

    if !output.status.success() {
        anyhow::bail!(
            "pdftohtml ist fehlgeschlagen: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let layout = fs::read_to_string(&xml_path)
        .with_context(|| format!("PDF-Layout konnte nicht gelesen werden: {}", xml_path.display()))?;
    parse_bold_short_texts(&layout)
}

fn parse_bold_short_texts(layout: &str) -> Result<HashMap<(String, usize), String>> {
    let font_re = Regex::new(r#"<fontspec\b(?P<attrs>[^>]*)/?>"#)?;
    let page_re = Regex::new(r#"(?s)<page\b(?P<attrs>[^>]*)>(?P<body>.*?)</page>"#)?;
    let text_re = Regex::new(r#"(?s)<text\b(?P<attrs>[^>]*)>(?P<body>.*?)</text>"#)?;
    let tag_re = Regex::new(r#"<[^>]+>"#)?;
    let position_re = Regex::new(r#"^(?P<oz>\d{2}\.\d{2}\.\d{2}\.\d{3})(?:\s+(?P<rest>.*))?$"#)?;
    let heading_re = Regex::new(r#"^\d{2}(?:\.\d{2}){0,2}\s+\S"#)?;

    let mut bold_fonts = HashSet::new();
    for caps in font_re.captures_iter(layout) {
        let attrs = caps.name("attrs").map(|value| value.as_str()).unwrap_or_default();
        let Some(id) = attr(attrs, "id") else { continue };
        let family = attr(attrs, "family").unwrap_or_default().to_lowercase();
        if ["bold", "semibold", "demi", "black", "heavy"]
            .iter()
            .any(|needle| family.contains(needle))
        {
            bold_fonts.insert(id);
        }
    }

    let mut result = HashMap::new();
    for page_caps in page_re.captures_iter(layout) {
        let attrs = page_caps.name("attrs").map(|value| value.as_str()).unwrap_or_default();
        let Some(page) = attr(attrs, "number").and_then(|value| value.parse::<usize>().ok()) else {
            continue;
        };
        let body = page_caps.name("body").map(|value| value.as_str()).unwrap_or_default();
        let mut fragments = Vec::new();

        for caps in text_re.captures_iter(body) {
            let attrs = caps.name("attrs").map(|value| value.as_str()).unwrap_or_default();
            let top = attr(attrs, "top").and_then(|v| v.parse().ok()).unwrap_or_default();
            let left = attr(attrs, "left").and_then(|v| v.parse().ok()).unwrap_or_default();
            let font = attr(attrs, "font").unwrap_or_default();
            let raw = caps.name("body").map(|value| value.as_str()).unwrap_or_default();
            let stripped = tag_re.replace_all(raw, "");
            let text = unescape(&stripped.replace("&nbsp;", " "))
                .map(|value| normalize(&value))
                .unwrap_or_else(|_| normalize(&stripped));
            if !text.is_empty() {
                fragments.push(TextFragment {
                    top,
                    left,
                    bold: bold_fonts.contains(&font),
                    text,
                });
            }
        }

        let lines = group_lines(fragments);
        for (index, line) in lines.iter().enumerate() {
            let Some(caps) = position_re.captures(&line.text) else { continue };
            let oz = caps.name("oz").unwrap().as_str().to_owned();
            let rest = caps.name("rest").map(|v| v.as_str()).unwrap_or_default();
            if is_reference_text(rest) {
                continue;
            }

            let mut short = Vec::new();
            if line.bold && is_short_text_candidate(rest) {
                short.push(rest.to_owned());
            }

            let mut started = !short.is_empty();
            for next in lines.iter().skip(index + 1).take(14) {
                if position_re.is_match(&next.text) || heading_re.is_match(&next.text) {
                    break;
                }
                if is_layout_noise(&next.text) || is_price_line(&next.text) {
                    if started && !is_layout_noise(&next.text) {
                        break;
                    }
                    continue;
                }
                if next.bold && is_short_text_candidate(&next.text) {
                    short.push(next.text.clone());
                    started = true;
                    continue;
                }
                if started {
                    break;
                }
            }

            if !short.is_empty() {
                let value = short.join("\n");
                result
                    .entry((oz, page))
                    .and_modify(|existing: &mut String| {
                        if value.lines().count() > existing.lines().count() {
                            *existing = value.clone();
                        }
                    })
                    .or_insert(value);
            }
        }
    }
    Ok(result)
}

fn group_lines(mut fragments: Vec<TextFragment>) -> Vec<LayoutLine> {
    fragments.sort_by_key(|fragment| (fragment.top, fragment.left));
    let mut lines: Vec<LayoutLine> = Vec::new();
    for fragment in fragments {
        if let Some(line) = lines.last_mut().filter(|line| (line.top - fragment.top).abs() <= 2) {
            if !line.text.is_empty() {
                line.text.push(' ');
            }
            line.text.push_str(&fragment.text);
            line.bold |= fragment.bold;
        } else {
            lines.push(LayoutLine {
                top: fragment.top,
                bold: fragment.bold,
                text: fragment.text,
            });
        }
    }
    for line in &mut lines {
        line.text = normalize(&line.text);
    }
    lines
}

fn is_short_text_candidate(value: &str) -> bool {
    !value.trim().is_empty()
        && !is_reference_text(value)
        && !is_price_line(value)
        && !is_layout_noise(value)
}

fn is_price_line(value: &str) -> bool {
    let lower = value.to_lowercase();
    lower.contains('€')
        || lower.contains("nur einh.-pr.")
        || Regex::new(r#"^[\d.]+,\d{2,3}\s+\S+"#)
            .map(|regex| regex.is_match(value.trim()))
            .unwrap_or(false)
}

fn is_layout_noise(value: &str) -> bool {
    matches!(
        value.trim(),
        "Fortsetzung von vorheriger Seite"
            | "Fortsetzung auf nächster Seite"
            | "Eventualposition ohne GB"
    ) || value.starts_with("OZ Menge / Einheit")
}

fn apply_bold_short_texts(
    nodes: &mut [Node],
    short_texts: &HashMap<(String, usize), String>,
) {
    for node in nodes {
        for position in &mut node.positions {
            let page = position.page_from.unwrap_or_default();
            let Some(short) = short_texts.get(&(position.oz.clone(), page)) else {
                continue;
            };
            remove_short_continuation_from_long_text(position, short);
            position.short_text = short.clone();
        }
        apply_bold_short_texts(&mut node.children, short_texts);
    }
}

fn remove_short_continuation_from_long_text(position: &mut Position, short: &str) {
    let continuation = short.lines().skip(1).map(normalize).collect::<Vec<_>>();
    if continuation.is_empty() {
        return;
    }
    let mut long_lines = position.long_text.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut removed = 0usize;
    while removed < continuation.len()
        && removed < long_lines.len()
        && normalize(&long_lines[removed]) == continuation[removed]
    {
        removed += 1;
    }
    if removed > 0 {
        long_lines.drain(..removed);
        position.long_text = long_lines.join("\n").trim().to_owned();
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
    value
        .replace('\u{00A0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_reference_positions() {
        let mut nodes = vec![Node {
            positions: vec![
                Position { oz: "01.01.01.010".into(), short_text: "Leistung".into(), ..Position::default() },
                Position { oz: "01.01.01.010".into(), short_text: "Zulage zu Position 01.01.01.010".into(), ..Position::default() },
            ],
            ..Node::default()
        }];
        remove_reference_positions(&mut nodes);
        assert_eq!(nodes[0].positions.len(), 1);
        assert_eq!(nodes[0].positions[0].short_text, "Leistung");
    }

    #[test]
    fn keeps_all_bold_short_text_lines() {
        let xml = r#"<pdf2xml>
          <fontspec id="0" family="Arial-BoldMT"/>
          <fontspec id="1" family="ArialMT"/>
          <page number="1">
            <text top="100" left="10" font="1">01.01.01.010 1,000 St 5,00 € 5,00 €</text>
            <text top="120" left="100" font="0">Erste Zeile des Kurztexts</text>
            <text top="140" left="100" font="0">zweite Zeile des Kurztexts</text>
            <text top="170" left="100" font="1">Normaler Langtext</text>
          </page>
        </pdf2xml>"#;
        let result = parse_bold_short_texts(xml).unwrap();
        assert_eq!(
            result.get(&("01.01.01.010".into(), 1)).map(String::as_str),
            Some("Erste Zeile des Kurztexts\nzweite Zeile des Kurztexts")
        );
    }
}
