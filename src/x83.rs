use std::{collections::HashSet, fs::File, io::BufWriter, path::Path, sync::LazyLock};

use anyhow::{bail, Result};
use chrono::Local;
use quick_xml::{
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    Writer,
};
use regex::Regex;

use crate::model::{BillOfQuantities, Node, Position};

const NS: &str = "http://www.gaeb.de/GAEB_DA_XML/DA83/3.3";

static TRAILING_TOTAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(?:Summe\s+)?[\d.]+,\d{2}(?:\s*(?:€|EUR))?\s*$")
        .expect("valid trailing total regex")
});

/// Writes a GAEB DA XML 3.3 (Ausgabe 2021-05) Angebotsaufforderung.
///
/// By default unresolved duplicate OZs and incomplete positions stop the export.
/// Set `allow_conflicts` only after a manual review of the parser warnings.
pub fn write_x83(
    boq: &BillOfQuantities,
    path: impl AsRef<Path>,
    allow_conflicts: bool,
) -> Result<()> {
    let conflicts = x83_conflicts(boq);
    if !allow_conflicts && !conflicts.is_empty() {
        bail!(
            "X83-Export gesperrt: {} Konflikt(e) müssen manuell geprüft werden:\n- {}\nDanach erneut mit --allow-conflicts exportieren.",
            conflicts.len(),
            conflicts.join("\n- ")
        );
    }

    let file = File::create(path)?;
    let mut writer = Writer::new_with_indent(BufWriter::new(file), b' ', 2);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let mut root = BytesStart::new("GAEB");
    root.push_attribute(("xmlns", NS));
    writer.write_event(Event::Start(root))?;

    write_gaeb_info(&mut writer)?;
    write_project_info(&mut writer, boq)?;

    writer.write_event(Event::Start(BytesStart::new("Award")))?;
    write_text(&mut writer, "DP", "83")?;

    writer.write_event(Event::Start(BytesStart::new("AwardInfo")))?;
    write_text(&mut writer, "Cur", &boq.currency)?;
    write_text(&mut writer, "CurLbl", currency_label(&boq.currency))?;
    writer.write_event(Event::End(BytesEnd::new("AwardInfo")))?;

    if !boq.preamble.trim().is_empty() {
        write_add_text(&mut writer, &boq.preamble)?;
    }

    let mut ids = IdGenerator::default();
    let boq_id = ids.next();
    let mut boq_start = BytesStart::new("BoQ");
    boq_start.push_attribute(("ID", boq_id.as_str()));
    writer.write_event(Event::Start(boq_start))?;

    write_boq_info(&mut writer, boq)?;
    writer.write_event(Event::Start(BytesStart::new("BoQBody")))?;
    for node in &boq.roots {
        write_category(&mut writer, node, &mut ids)?;
    }
    writer.write_event(Event::End(BytesEnd::new("BoQBody")))?;
    writer.write_event(Event::End(BytesEnd::new("BoQ")))?;
    writer.write_event(Event::End(BytesEnd::new("Award")))?;
    writer.write_event(Event::End(BytesEnd::new("GAEB")))?;
    Ok(())
}

pub fn x83_conflicts(boq: &BillOfQuantities) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut conflicts = Vec::new();
    collect_conflicts(&boq.roots, &mut seen, &mut conflicts);
    conflicts
}

fn collect_conflicts(nodes: &[Node], seen: &mut HashSet<String>, conflicts: &mut Vec<String>) {
    for node in nodes {
        for position in &node.positions {
            if !seen.insert(position.oz.clone()) {
                conflicts.push(format!("Doppelte OZ: {}", position.oz));
            }
            if position.quantity.is_none() {
                conflicts.push(format!("Menge fehlt: {}", position.oz));
            }
            if position.unit.as_deref().unwrap_or_default().trim().is_empty() {
                conflicts.push(format!("Einheit fehlt: {}", position.oz));
            }
            if position.short_text.trim().is_empty() && position.long_text.trim().is_empty() {
                conflicts.push(format!("Positionstext fehlt: {}", position.oz));
            }
        }
        collect_conflicts(&node.children, seen, conflicts);
    }
}

fn write_gaeb_info<W: std::io::Write>(writer: &mut Writer<W>) -> Result<()> {
    let now = Local::now();
    writer.write_event(Event::Start(BytesStart::new("GAEBInfo")))?;
    write_text(writer, "Version", "3.3")?;
    write_text(writer, "VersDate", "2021-05")?;
    write_text(writer, "Date", &now.format("%Y-%m-%d").to_string())?;
    write_text(writer, "Time", &now.format("%H:%M:%S").to_string())?;
    write_text(writer, "ProgSystem", "gaeb-toolkit")?;
    write_text(writer, "ProgName", "gaeb-toolkit")?;
    write_text(writer, "Certific", "not certified")?;
    writer.write_event(Event::End(BytesEnd::new("GAEBInfo")))?;
    Ok(())
}

fn write_project_info<W: std::io::Write>(
    writer: &mut Writer<W>,
    boq: &BillOfQuantities,
) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new("PrjInfo")))?;
    write_text(writer, "NamePrj", project_number(boq))?;
    write_text(writer, "LblPrj", project_label(boq))?;
    write_text(writer, "Cur", &boq.currency)?;
    write_text(writer, "CurLbl", currency_label(&boq.currency))?;
    writer.write_event(Event::End(BytesEnd::new("PrjInfo")))?;
    Ok(())
}

fn write_boq_info<W: std::io::Write>(writer: &mut Writer<W>, boq: &BillOfQuantities) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new("BoQInfo")))?;
    write_text(writer, "Name", "01")?;
    write_text(writer, "LblBoQ", project_label(boq))?;
    write_text(writer, "OutlCompl", "AllTxt")?;

    for (label, length) in [("Bereich", 2), ("Titel", 2), ("Untertitel", 2)] {
        writer.write_event(Event::Start(BytesStart::new("BoQBkdn")))?;
        write_text(writer, "Type", "BoQLevel")?;
        write_text(writer, "LblBoQBkdn", label)?;
        write_text(writer, "Length", &length.to_string())?;
        write_text(writer, "Num", "Yes")?;
        writer.write_event(Event::End(BytesEnd::new("BoQBkdn")))?;
    }

    writer.write_event(Event::Start(BytesStart::new("BoQBkdn")))?;
    write_text(writer, "Type", "Item")?;
    write_text(writer, "Length", "3")?;
    write_text(writer, "Num", "Yes")?;
    writer.write_event(Event::End(BytesEnd::new("BoQBkdn")))?;
    writer.write_event(Event::End(BytesEnd::new("BoQInfo")))?;
    Ok(())
}

fn write_category<W: std::io::Write>(
    writer: &mut Writer<W>,
    node: &Node,
    ids: &mut IdGenerator,
) -> Result<()> {
    let id = ids.next();
    let rno = node.oz.rsplit('.').next().unwrap_or(&node.oz);
    let mut start = BytesStart::new("BoQCtgy");
    start.push_attribute(("ID", id.as_str()));
    start.push_attribute(("RNoPart", rno));
    writer.write_event(Event::Start(start))?;

    let clean_title = strip_trailing_totals(&node.title);
    write_rich_text(writer, "LblTx", &clean_title)?;
    writer.write_event(Event::Start(BytesStart::new("BoQBody")))?;

    for child in &node.children {
        write_category(writer, child, ids)?;
    }

    if !node.positions.is_empty() {
        writer.write_event(Event::Start(BytesStart::new("Itemlist")))?;
        for position in &node.positions {
            write_item(writer, position, ids)?;
        }
        writer.write_event(Event::End(BytesEnd::new("Itemlist")))?;
    }

    writer.write_event(Event::End(BytesEnd::new("BoQBody")))?;
    writer.write_event(Event::End(BytesEnd::new("BoQCtgy")))?;
    Ok(())
}

fn strip_trailing_totals(value: &str) -> String {
    let mut result = value.trim().to_owned();
    loop {
        let cleaned = TRAILING_TOTAL_RE.replace(&result, "").trim().to_owned();
        if cleaned == result {
            break;
        }
        result = cleaned;
    }
    result
}

fn write_item<W: std::io::Write>(
    writer: &mut Writer<W>,
    position: &Position,
    ids: &mut IdGenerator,
) -> Result<()> {
    let id = ids.next();
    let rno = position.oz.rsplit('.').next().unwrap_or(&position.oz);
    let mut start = BytesStart::new("Item");
    start.push_attribute(("ID", id.as_str()));
    start.push_attribute(("RNoPart", rno));
    writer.write_event(Event::Start(start))?;

    if let Some(quantity) = position.quantity {
        write_text(writer, "Qty", &quantity.normalize().to_string())?;
    }
    if let Some(unit) = position.unit.as_deref().filter(|value| !value.trim().is_empty()) {
        write_text(writer, "QU", unit)?;
    }

    writer.write_event(Event::Start(BytesStart::new("Description")))?;
    writer.write_event(Event::Start(BytesStart::new("CompleteText")))?;

    if !position.long_text.trim().is_empty() {
        writer.write_event(Event::Start(BytesStart::new("DetailTxt")))?;
        write_text_block(writer, "Text", &position.long_text)?;
        writer.write_event(Event::End(BytesEnd::new("DetailTxt")))?;
    }

    writer.write_event(Event::Start(BytesStart::new("OutlineText")))?;
    writer.write_event(Event::Start(BytesStart::new("OutlTxt")))?;
    write_text_block(writer, "TextOutlTxt", &position.short_text)?;
    writer.write_event(Event::End(BytesEnd::new("OutlTxt")))?;
    writer.write_event(Event::End(BytesEnd::new("OutlineText")))?;

    writer.write_event(Event::End(BytesEnd::new("CompleteText")))?;
    writer.write_event(Event::End(BytesEnd::new("Description")))?;
    writer.write_event(Event::End(BytesEnd::new("Item")))?;
    Ok(())
}

fn write_add_text<W: std::io::Write>(writer: &mut Writer<W>, value: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new("AddText")))?;
    writer.write_event(Event::Start(BytesStart::new("OutlineAddText")))?;
    write_text(writer, "span", "Vorbemerkungen")?;
    writer.write_event(Event::End(BytesEnd::new("OutlineAddText")))?;
    write_text_block(writer, "DetailAddText", value)?;
    writer.write_event(Event::End(BytesEnd::new("AddText")))?;
    Ok(())
}

fn write_rich_text<W: std::io::Write>(writer: &mut Writer<W>, name: &str, value: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    writer.write_event(Event::Start(BytesStart::new("p")))?;
    write_text(writer, "span", value)?;
    writer.write_event(Event::End(BytesEnd::new("p")))?;
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

fn write_text_block<W: std::io::Write>(writer: &mut Writer<W>, name: &str, value: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    let lines = value.lines().filter(|line| !line.trim().is_empty()).collect::<Vec<_>>();
    if lines.is_empty() {
        writer.write_event(Event::Start(BytesStart::new("p")))?;
        write_text(writer, "span", "")?;
        writer.write_event(Event::End(BytesEnd::new("p")))?;
    } else {
        for line in lines {
            writer.write_event(Event::Start(BytesStart::new("p")))?;
            write_text(writer, "span", line.trim())?;
            writer.write_event(Event::End(BytesEnd::new("p")))?;
        }
    }
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

fn write_text<W: std::io::Write>(writer: &mut Writer<W>, name: &str, value: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    writer.write_event(Event::Text(BytesText::new(value)))?;
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

fn project_number(boq: &BillOfQuantities) -> &str {
    if boq.project.trim().is_empty() { "01" } else { &boq.project }
}

fn project_label(boq: &BillOfQuantities) -> &str {
    if boq.project.trim().is_empty() { &boq.source } else { &boq.project }
}

fn currency_label(currency: &str) -> &str {
    match currency {
        "EUR" => "Euro",
        _ => currency,
    }
}

#[derive(Default)]
struct IdGenerator(u64);

impl IdGenerator {
    fn next(&mut self) -> String {
        self.0 += 1;
        format!("id{:08}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rust_decimal::Decimal;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn writes_x83_with_position_hierarchy() {
        let mut boq = BillOfQuantities::new("angebot.pdf");
        boq.project = "Rue 7".into();
        boq.roots.push(Node {
            oz: "01".into(),
            title: "Bereich".into(),
            level: 1,
            children: vec![Node {
                oz: "01.02".into(),
                title: "Titel".into(),
                level: 2,
                children: vec![Node {
                    oz: "01.02.03".into(),
                    title: "Untertitel".into(),
                    level: 3,
                    positions: vec![Position {
                        oz: "01.02.03.040".into(),
                        quantity: Some(Decimal::new(12500, 3)),
                        unit: Some("St".into()),
                        short_text: "Kurztext".into(),
                        long_text: "Langtext".into(),
                        ..Position::default()
                    }],
                    ..Node::default()
                }],
                ..Node::default()
            }],
            ..Node::default()
        });

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.x83");
        write_x83(&boq, &path, false).unwrap();
        let xml = fs::read_to_string(path).unwrap();
        assert!(xml.contains("http://www.gaeb.de/GAEB_DA_XML/DA83/3.3"));
        assert!(xml.contains("<DP>83</DP>"));
        assert!(xml.contains("RNoPart=\"040\""));
        assert!(xml.contains("<Qty>12.5</Qty>"));
        assert!(!xml.contains("<UP>"));
    }

    #[test]
    fn removes_totals_from_category_labels() {
        assert_eq!(
            strip_trailing_totals("Rückbauarbeiten 123.456,78 €"),
            "Rückbauarbeiten"
        );
        assert_eq!(
            strip_trailing_totals("Schutzmaßnahmen 1.000,00 EUR 2.000,00 €"),
            "Schutzmaßnahmen"
        );
        assert_eq!(strip_trailing_totals("Titel 2026"), "Titel 2026");
    }

    #[test]
    fn blocks_duplicate_oz_without_manual_approval() {
        let mut boq = BillOfQuantities::new("angebot.pdf");
        let duplicate = Position {
            oz: "01.01.01.010".into(),
            quantity: Some(Decimal::ONE),
            unit: Some("St".into()),
            short_text: "Test".into(),
            ..Position::default()
        };
        boq.roots.push(Node {
            oz: "01".into(),
            level: 1,
            children: vec![Node {
                oz: "01.01".into(),
                level: 2,
                children: vec![Node {
                    oz: "01.01.01".into(),
                    level: 3,
                    positions: vec![duplicate.clone(), duplicate],
                    ..Node::default()
                }],
                ..Node::default()
            }],
            ..Node::default()
        });

        let dir = tempdir().unwrap();
        let error = write_x83(&boq, dir.path().join("test.x83"), false).unwrap_err();
        assert!(error.to_string().contains("manuell geprüft"));
    }
}
