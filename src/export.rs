use std::{fs::File, io::BufWriter, path::Path};

use anyhow::Result;
use quick_xml::{events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event}, Writer};

use crate::model::{BillOfQuantities, Node, Position};

pub fn write_json(boq: &BillOfQuantities, path: impl AsRef<Path>) -> Result<()> {
    let file = File::create(path)?;
    serde_json::to_writer_pretty(BufWriter::new(file), boq)?;
    Ok(())
}

pub fn write_master_xml(boq: &BillOfQuantities, path: impl AsRef<Path>) -> Result<()> {
    let file = File::create(path)?;
    let mut writer = Writer::new_with_indent(BufWriter::new(file), b' ', 2);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let mut root = BytesStart::new("Leistungsverzeichnis");
    root.push_attribute(("version", "0.1"));
    root.push_attribute(("currency", boq.currency.as_str()));
    writer.write_event(Event::Start(root))?;

    writer.write_event(Event::Start(BytesStart::new("Metadaten")))?;
    write_text(&mut writer, "Quelldatei", &boq.source)?;
    write_text(&mut writer, "Projekt", &boq.project)?;
    write_text(&mut writer, "Auftraggeber", &boq.client)?;
    write_text(&mut writer, "Bieter", &boq.bidder)?;
    writer.write_event(Event::End(BytesEnd::new("Metadaten")))?;

    if !boq.preamble.is_empty() {
        write_text(&mut writer, "Vorbemerkungen", &boq.preamble)?;
    }

    writer.write_event(Event::Start(BytesStart::new("Hierarchie")))?;
    for node in &boq.roots {
        write_node(&mut writer, node)?;
    }
    writer.write_event(Event::End(BytesEnd::new("Hierarchie")))?;

    if !boq.warnings.is_empty() {
        writer.write_event(Event::Start(BytesStart::new("Validierung")))?;
        for warning in &boq.warnings {
            write_text(&mut writer, "Warnung", warning)?;
        }
        writer.write_event(Event::End(BytesEnd::new("Validierung")))?;
    }

    writer.write_event(Event::End(BytesEnd::new("Leistungsverzeichnis")))?;
    Ok(())
}

fn write_node<W: std::io::Write>(writer: &mut Writer<W>, node: &Node) -> Result<()> {
    let name = match node.level {
        1 => "Bereich",
        2 => "Titel",
        3 => "Untertitel",
        _ => "Gliederung",
    };
    let page = node.page.map(|v| v.to_string()).unwrap_or_default();
    let mut element = BytesStart::new(name);
    element.push_attribute(("oz", node.oz.as_str()));
    element.push_attribute(("seite", page.as_str()));
    writer.write_event(Event::Start(element))?;
    write_text(writer, "Bezeichnung", &node.title)?;
    for position in &node.positions {
        write_position(writer, position)?;
    }
    for child in &node.children {
        write_node(writer, child)?;
    }
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

fn write_position<W: std::io::Write>(writer: &mut Writer<W>, position: &Position) -> Result<()> {
    let page_from = position.page_from.map(|v| v.to_string()).unwrap_or_default();
    let page_to = position.page_to.map(|v| v.to_string()).unwrap_or_default();
    let mut element = BytesStart::new("Position");
    element.push_attribute(("oz", position.oz.as_str()));
    element.push_attribute(("seiteVon", page_from.as_str()));
    element.push_attribute(("seiteBis", page_to.as_str()));
    element.push_attribute(("eventual", if position.provisional { "true" } else { "false" }));
    element.push_attribute(("nurEinheitspreis", if position.price_only { "true" } else { "false" }));
    writer.write_event(Event::Start(element))?;
    write_text(writer, "Kurztext", &position.short_text)?;
    write_text(writer, "Langtext", &position.long_text)?;

    let mut quantity = BytesStart::new("Menge");
    quantity.push_attribute(("einheit", position.unit.as_deref().unwrap_or_default()));
    writer.write_event(Event::Start(quantity))?;
    writer.write_event(Event::Text(BytesText::new(&position.quantity.map(|v| v.to_string()).unwrap_or_default())))?;
    writer.write_event(Event::End(BytesEnd::new("Menge")))?;

    let mut prices = BytesStart::new("Preise");
    prices.push_attribute(("waehrung", "EUR"));
    writer.write_event(Event::Start(prices))?;
    write_text(writer, "Einheitspreis", &position.unit_price.map(|v| v.to_string()).unwrap_or_default())?;
    write_text(writer, "Gesamtbetrag", &position.total_price.map(|v| v.to_string()).unwrap_or_default())?;
    writer.write_event(Event::End(BytesEnd::new("Preise")))?;
    writer.write_event(Event::End(BytesEnd::new("Position")))?;
    Ok(())
}

fn write_text<W: std::io::Write>(writer: &mut Writer<W>, name: &str, value: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    writer.write_event(Event::Text(BytesText::new(value)))?;
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}
