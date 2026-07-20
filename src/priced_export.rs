use std::{fs::File, io::BufWriter, path::Path};

use anyhow::{bail, Result};
use chrono::Local;
use quick_xml::{
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    Writer,
};
use rust_decimal::Decimal;

use crate::{model::{BillOfQuantities, Node, Position}, x83_conflicts};

#[derive(Clone, Copy)]
enum Phase {
    X83,
    X84,
}

impl Phase {
    fn dp(self) -> &'static str {
        match self {
            Self::X83 => "83",
            Self::X84 => "84",
        }
    }

    fn namespace(self) -> &'static str {
        match self {
            Self::X83 => "http://www.gaeb.de/GAEB_DA_XML/DA83/3.3",
            Self::X84 => "http://www.gaeb.de/GAEB_DA_XML/DA84/3.3",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::X83 => "X83 mit Preisen",
            Self::X84 => "X84",
        }
    }
}

pub fn write_x84(
    boq: &BillOfQuantities,
    path: impl AsRef<Path>,
    allow_conflicts: bool,
) -> Result<()> {
    write_priced(boq, path, allow_conflicts, Phase::X84)
}

/// Nicht standardmäßige, aber von einigen AVA-Systemen nutzbare X83-Ausgabe
/// mit UP/IT. Für den regulären Angebotsrücklauf ist X84 zu verwenden.
pub fn write_x83_priced(
    boq: &BillOfQuantities,
    path: impl AsRef<Path>,
    allow_conflicts: bool,
) -> Result<()> {
    write_priced(boq, path, allow_conflicts, Phase::X83)
}

fn write_priced(
    boq: &BillOfQuantities,
    path: impl AsRef<Path>,
    allow_conflicts: bool,
    phase: Phase,
) -> Result<()> {
    let conflicts = priced_conflicts(boq);
    if !allow_conflicts && !conflicts.is_empty() {
        bail!(
            "{}-Export gesperrt: {} Konflikt(e) müssen manuell geprüft werden:\n- {}\nDanach erneut mit --allow-conflicts exportieren.",
            phase.label(),
            conflicts.len(),
            conflicts.join("\n- ")
        );
    }

    let file = File::create(path)?;
    let mut writer = Writer::new_with_indent(BufWriter::new(file), b' ', 2);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let mut root = BytesStart::new("GAEB");
    root.push_attribute(("xmlns", phase.namespace()));
    writer.write_event(Event::Start(root))?;

    write_gaeb_info(&mut writer)?;
    write_project_info(&mut writer, boq)?;

    writer.write_event(Event::Start(BytesStart::new("Award")))?;
    write_text(&mut writer, "DP", phase.dp())?;

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

fn priced_conflicts(boq: &BillOfQuantities) -> Vec<String> {
    let mut conflicts = x83_conflicts(boq);
    collect_price_conflicts(&boq.roots, &mut conflicts);
    conflicts
}

fn collect_price_conflicts(nodes: &[Node], conflicts: &mut Vec<String>) {
    for node in nodes {
        for position in &node.positions {
            if is_omitted(position) || position.price_only {
                continue;
            }
            if position.unit_price.is_none() {
                conflicts.push(format!("EP fehlt: {}", position.oz));
            }
            if position.total_price.is_none() {
                conflicts.push(format!("GP fehlt: {}", position.oz));
            }
        }
        collect_price_conflicts(&node.children, conflicts);
    }
}

fn is_omitted(position: &Position) -> bool {
    position
        .short_text
        .lines()
        .chain(position.long_text.lines())
        .any(|line| line.trim().eq_ignore_ascii_case("Position entfällt"))
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

fn write_project_info<W: std::io::Write>(writer: &mut Writer<W>, boq: &BillOfQuantities) -> Result<()> {
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

fn write_category<W: std::io::Write>(writer: &mut Writer<W>, node: &Node, ids: &mut IdGenerator) -> Result<()> {
    let id = ids.next();
    let rno = node.oz.rsplit('.').next().unwrap_or(&node.oz);
    let mut start = BytesStart::new("BoQCtgy");
    start.push_attribute(("ID", id.as_str()));
    start.push_attribute(("RNoPart", rno));
    writer.write_event(Event::Start(start))?;
    write_rich_text(writer, "LblTx", node.title.trim())?;
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

fn write_item<W: std::io::Write>(writer: &mut Writer<W>, position: &Position, ids: &mut IdGenerator) -> Result<()> {
    let id = ids.next();
    let rno = position.oz.rsplit('.').next().unwrap_or(&position.oz);
    let mut start = BytesStart::new("Item");
    start.push_attribute(("ID", id.as_str()));
    start.push_attribute(("RNoPart", rno));
    writer.write_event(Event::Start(start))?;

    if let Some(quantity) = position.quantity {
        write_decimal(writer, "Qty", quantity, 3)?;
    }
    if let Some(unit) = position.unit.as_deref().filter(|value| !value.trim().is_empty()) {
        write_text(writer, "QU", unit)?;
    }
    if let Some(unit_price) = position.unit_price {
        write_decimal(writer, "UP", unit_price, 2)?;
    }
    if let Some(total_price) = position.total_price {
        write_decimal(writer, "IT", total_price, 2)?;
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

fn write_decimal<W: std::io::Write>(writer: &mut Writer<W>, name: &str, value: Decimal, scale: u32) -> Result<()> {
    write_text(writer, name, &value.round_dp(scale).normalize().to_string())
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
    use tempfile::tempdir;
    use super::*;

    fn priced_boq() -> BillOfQuantities {
        let mut boq = BillOfQuantities::new("angebot.pdf");
        boq.roots.push(Node {
            oz: "01".into(),
            children: vec![Node {
                oz: "01.01".into(),
                children: vec![Node {
                    oz: "01.01.01".into(),
                    positions: vec![Position {
                        oz: "01.01.01.010".into(),
                        quantity: Some(Decimal::new(2500, 3)),
                        unit: Some("St".into()),
                        unit_price: Some(Decimal::new(1000, 2)),
                        total_price: Some(Decimal::new(2500, 2)),
                        short_text: "Leistung".into(),
                        ..Position::default()
                    }],
                    ..Node::default()
                }],
                ..Node::default()
            }],
            ..Node::default()
        });
        boq
    }

    #[test]
    fn writes_x84_with_prices() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.x84");
        write_x84(&priced_boq(), &path, false).unwrap();
        let xml = fs::read_to_string(path).unwrap();
        assert!(xml.contains("DA84/3.3"));
        assert!(xml.contains("<DP>84</DP>"));
        assert!(xml.contains("<UP>10</UP>"));
        assert!(xml.contains("<IT>25</IT>"));
    }

    #[test]
    fn writes_priced_x83() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.x83");
        write_x83_priced(&priced_boq(), &path, false).unwrap();
        let xml = fs::read_to_string(path).unwrap();
        assert!(xml.contains("DA83/3.3"));
        assert!(xml.contains("<DP>83</DP>"));
        assert!(xml.contains("<UP>10</UP>"));
    }
}
