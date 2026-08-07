use std::{
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
    str::FromStr,
};

use anyhow::{bail, Context, Result};
use printpdf::{
    BuiltinFont, Color, Greyscale, IndirectFontRef, Mm, PdfDocument, PdfDocumentReference,
    PdfLayerReference,
};
use quick_xml::{events::Event, Reader};
use rust_decimal::Decimal;

use crate::CONVERTER_BRANDING;

#[derive(Debug, Clone, PartialEq)]
pub struct GaebDocument {
    pub source: String,
    pub project: String,
    pub boq: String,
    pub exchange_phase: String,
    pub currency: String,
    pub rows: Vec<GaebRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GaebRow {
    Category {
        oz: String,
        title: String,
        level: usize,
    },
    Item(GaebItem),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GaebItem {
    pub oz: String,
    pub short_text: String,
    pub long_text: String,
    pub quantity: Option<Decimal>,
    pub unit: String,
    pub unit_price: Option<Decimal>,
    pub total_price: Option<Decimal>,
}

pub fn read_gaeb_xml(path: impl AsRef<Path>) -> Result<GaebDocument> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| {
        format!(
            "GAEB-Datei konnte nicht geöffnet werden: {}",
            path.display()
        )
    })?;
    let mut reader = Reader::from_reader(BufReader::new(file));
    reader.config_mut().trim_text(true);

    let mut document = GaebDocument {
        source: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("GAEB-Datei")
            .to_owned(),
        project: String::new(),
        boq: String::new(),
        exchange_phase: String::new(),
        currency: "EUR".to_owned(),
        rows: Vec::new(),
    };
    let mut buffer = Vec::new();
    let mut elements = Vec::<String>::new();
    let mut categories = Vec::<String>::new();
    let mut active_category: Option<usize> = None;
    let mut item: Option<GaebItem> = None;
    let mut saw_gaeb = false;

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) => {
                let name = local_name(event.name().as_ref());
                saw_gaeb |= name == "GAEB";
                if name == "BoQCtgy" {
                    let part = attribute(&event, b"RNoPart")?.unwrap_or_default();
                    categories.push(part);
                    let index = document.rows.len();
                    document.rows.push(GaebRow::Category {
                        oz: categories.join("."),
                        title: String::new(),
                        level: categories.len(),
                    });
                    active_category = Some(index);
                } else if name == "Item" {
                    let part = attribute(&event, b"RNoPart")?.unwrap_or_default();
                    let mut oz_parts = categories.clone();
                    oz_parts.push(part);
                    item = Some(GaebItem {
                        oz: oz_parts.join("."),
                        ..GaebItem::default()
                    });
                }
                elements.push(name);
            }
            Event::Empty(event) => {
                saw_gaeb |= local_name(event.name().as_ref()) == "GAEB";
            }
            Event::Text(event) => {
                let text = event.unescape()?.trim().to_owned();
                if !text.is_empty() {
                    apply_text(
                        &mut document,
                        &elements,
                        active_category,
                        item.as_mut(),
                        &text,
                    );
                }
            }
            Event::CData(event) => {
                let text = event.decode()?.trim().to_owned();
                if !text.is_empty() {
                    apply_text(
                        &mut document,
                        &elements,
                        active_category,
                        item.as_mut(),
                        &text,
                    );
                }
            }
            Event::End(event) => {
                let name = local_name(event.name().as_ref());
                if name == "Item" {
                    if let Some(position) = item.take() {
                        document.rows.push(GaebRow::Item(position));
                    }
                } else if name == "BoQCtgy" {
                    categories.pop();
                    active_category = document.rows.iter().rposition(|row| {
                        matches!(
                            row,
                            GaebRow::Category { level, .. } if *level == categories.len()
                        )
                    });
                }
                elements.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    if !saw_gaeb {
        bail!("Die Datei ist kein unterstütztes GAEB-DA-XML-Dokument.");
    }
    let has_items = document
        .rows
        .iter()
        .any(|row| matches!(row, GaebRow::Item(_)));
    let is_structured_phase_81 = document.exchange_phase == "81"
        && document
            .rows
            .iter()
            .any(|row| matches!(row, GaebRow::Category { .. }));
    if !has_items && !is_structured_phase_81 {
        bail!("Die GAEB-Datei enthält keine lesbaren LV-Positionen.");
    }
    Ok(document)
}

fn apply_text(
    document: &mut GaebDocument,
    elements: &[String],
    active_category: Option<usize>,
    item: Option<&mut GaebItem>,
    text: &str,
) {
    let current = elements.last().map(String::as_str).unwrap_or_default();
    if elements.iter().any(|element| element == "Item") {
        if let Some(item) = item {
            if elements.iter().any(|element| element == "TextOutlTxt") {
                append_text(&mut item.short_text, text);
            } else if elements.iter().any(|element| element == "DetailTxt") {
                append_text(&mut item.long_text, text);
            } else {
                match current {
                    "Qty" => item.quantity = parse_decimal(text),
                    "QU" => item.unit = text.to_owned(),
                    "UP" => item.unit_price = parse_decimal(text),
                    "IT" => item.total_price = parse_decimal(text),
                    _ => {}
                }
            }
        }
        return;
    }

    if elements.iter().any(|element| element == "LblTx") {
        if let Some(GaebRow::Category { title, .. }) =
            active_category.and_then(|index| document.rows.get_mut(index))
        {
            append_text(title, text);
        }
        return;
    }

    match current {
        "LblPrj" => document.project = text.to_owned(),
        "NamePrj" if document.project.is_empty() => document.project = text.to_owned(),
        "LblBoQ" if document.boq.is_empty() => document.boq = text.to_owned(),
        "DP" if document.exchange_phase.is_empty() => document.exchange_phase = text.to_owned(),
        "Cur" => document.currency = text.to_owned(),
        _ => {}
    }
}

fn append_text(target: &mut String, value: &str) {
    if !target.is_empty() {
        target.push(' ');
    }
    target.push_str(value);
}

fn attribute(event: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for attribute in event.attributes().with_checks(false) {
        let attribute = attribute?;
        if local_name(attribute.key.as_ref()).as_bytes() == name {
            return Ok(Some(attribute.unescape_value()?.into_owned()));
        }
    }
    Ok(None)
}

fn local_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name)
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn parse_decimal(value: &str) -> Option<Decimal> {
    Decimal::from_str(value.trim().replace(',', ".").as_str()).ok()
}

pub fn write_gaeb_pdf(document: &GaebDocument, path: impl AsRef<Path>) -> Result<()> {
    let (pdf, page, layer) =
        PdfDocument::new("GAEB-Leistungsverzeichnis", Mm(210.0), Mm(297.0), "Inhalt");
    let regular = pdf.add_builtin_font(BuiltinFont::Helvetica)?;
    let bold = pdf.add_builtin_font(BuiltinFont::HelveticaBold)?;
    let mut renderer = PdfRenderer::new(pdf, page, layer, regular, bold);
    renderer.write_document(document)?;
    renderer
        .pdf
        .save(&mut BufWriter::new(File::create(path)?))?;
    Ok(())
}

struct PdfRenderer {
    pdf: PdfDocumentReference,
    page: printpdf::PdfPageIndex,
    layer: printpdf::PdfLayerIndex,
    regular: IndirectFontRef,
    bold: IndirectFontRef,
    y: f32,
    page_number: usize,
}

impl PdfRenderer {
    fn new(
        pdf: PdfDocumentReference,
        page: printpdf::PdfPageIndex,
        layer: printpdf::PdfLayerIndex,
        regular: IndirectFontRef,
        bold: IndirectFontRef,
    ) -> Self {
        Self {
            pdf,
            page,
            layer,
            regular,
            bold,
            y: 278.0,
            page_number: 1,
        }
    }

    fn write_document(&mut self, document: &GaebDocument) -> Result<()> {
        self.text(15.0, self.y, 17.0, &self.bold, "GAEB-Leistungsverzeichnis");
        self.y -= 8.0;
        self.text(
            15.0,
            self.y,
            9.0,
            &self.regular,
            &format!("Datei: {}", printable(&document.source)),
        );
        self.y -= 5.0;
        if !document.project.is_empty() {
            self.text(
                15.0,
                self.y,
                9.0,
                &self.regular,
                &format!("Projekt: {}", printable(&document.project)),
            );
            self.y -= 5.0;
        }
        if !document.boq.is_empty() && document.boq != document.project {
            self.text(
                15.0,
                self.y,
                9.0,
                &self.regular,
                &format!("LV: {}", printable(&document.boq)),
            );
            self.y -= 5.0;
        }
        self.text(
            15.0,
            self.y,
            9.0,
            &self.regular,
            &format!(
                "Austauschphase: X{}   Währung: {}",
                document.exchange_phase,
                printable(&document.currency)
            ),
        );
        self.y -= 10.0;
        self.table_header();

        for row in &document.rows {
            match row {
                GaebRow::Category { oz, title, level } => {
                    self.ensure_space(10.0);
                    self.y -= 2.0;
                    let indent = (*level as f32 - 1.0).max(0.0) * 4.0;
                    self.text(
                        15.0 + indent,
                        self.y,
                        10.5,
                        &self.bold,
                        &printable(&format!("{oz}  {title}")),
                    );
                    self.y -= 7.0;
                }
                GaebRow::Item(item) => self.write_item(item),
            }
        }
        self.footer();
        Ok(())
    }

    fn table_header(&mut self) {
        self.text(15.0, self.y, 8.0, &self.bold, "OZ");
        self.text(42.0, self.y, 8.0, &self.bold, "Leistungsbeschreibung");
        self.text(151.0, self.y, 8.0, &self.bold, "Menge / EH");
        self.text(176.0, self.y, 8.0, &self.bold, "Betrag");
        self.y -= 7.0;
    }

    fn write_item(&mut self, item: &GaebItem) {
        let short = if item.short_text.is_empty() {
            "(ohne Kurztext)"
        } else {
            &item.short_text
        };
        let short_lines = wrap(short, 72);
        let long_lines = wrap(&item.long_text, 88);
        let item_height =
            short_lines.len().max(1) as f32 * 4.5 + long_lines.len() as f32 * 4.2 + 5.0;
        if item_height < 245.0 {
            self.ensure_space(item_height);
        }
        self.text(15.0, self.y, 8.5, &self.bold, &printable(&item.oz));
        for (index, line) in short_lines.iter().enumerate() {
            self.text(
                42.0,
                self.y - index as f32 * 4.5,
                8.5,
                &self.bold,
                &printable(line),
            );
        }
        let quantity = item
            .quantity
            .map(|value| value.normalize().to_string())
            .unwrap_or_default();
        self.text(
            151.0,
            self.y,
            8.0,
            &self.regular,
            &printable(&format!("{quantity} {}", item.unit)),
        );
        let amount = item
            .total_price
            .or(item.unit_price)
            .map(|value| format!("{} {}", value.normalize(), "EUR"))
            .unwrap_or_default();
        self.text(176.0, self.y, 8.0, &self.regular, &amount);
        self.y -= short_lines.len().max(1) as f32 * 4.5 + 2.0;

        for line in long_lines {
            self.ensure_space(5.0);
            self.text(42.0, self.y, 7.8, &self.regular, &printable(&line));
            self.y -= 4.2;
        }
        self.y -= 3.0;
    }

    fn ensure_space(&mut self, needed: f32) {
        if self.y - needed >= 18.0 {
            return;
        }
        self.footer();
        let (page, layer) = self.pdf.add_page(Mm(210.0), Mm(297.0), "Inhalt");
        self.page = page;
        self.layer = layer;
        self.page_number += 1;
        self.y = 278.0;
        self.table_header();
    }

    fn footer(&self) {
        let layer: PdfLayerReference = self.pdf.get_page(self.page).get_layer(self.layer);
        layer.set_fill_color(Color::Greyscale(Greyscale::new(0.48, None)));
        layer.use_text(CONVERTER_BRANDING, 6.5, Mm(15.0), Mm(10.0), &self.regular);
        self.text(
            174.0,
            10.0,
            7.5,
            &self.regular,
            &format!("Seite {}", self.page_number),
        );
    }

    fn text(&self, x: f32, y: f32, size: f32, font: &IndirectFontRef, value: &str) {
        let layer: PdfLayerReference = self.pdf.get_page(self.page).get_layer(self.layer);
        layer.use_text(value, size, Mm(x), Mm(y), font);
    }
}

fn wrap(value: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in value.lines() {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > max_chars
            {
                lines.push(current);
                current = String::new();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}

fn printable(value: &str) -> String {
    value
        .replace(['–', '—'], "-")
        .replace(['“', '”'], "\"")
        .replace('’', "'")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{read_gaeb_xml, write_gaeb_pdf, GaebRow};

    #[test]
    fn reads_gaeb_xml_and_writes_pdf() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("test.x84");
        let output = directory.path().join("test.pdf");
        fs::write(
            &input,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<GAEB xmlns="http://www.gaeb.de/GAEB_DA_XML/DA84/3.3">
  <PrjInfo><LblPrj>Umbau Büro</LblPrj></PrjInfo>
  <Award><DP>84</DP><AwardInfo><Cur>EUR</Cur></AwardInfo>
    <BoQ><BoQInfo><LblBoQ>Malerarbeiten</LblBoQ></BoQInfo><BoQBody>
      <BoQCtgy RNoPart="01"><LblTx><p><span>Titel</span></p></LblTx><BoQBody>
        <Itemlist><Item RNoPart="001"><Qty>12.5</Qty><QU>m²</QU><UP>4.2</UP><IT>52.5</IT>
          <Description><CompleteText><DetailTxt><Text><p><span>Untergrund vorbereiten.</span></p></Text></DetailTxt>
          <OutlineText><OutlTxt><TextOutlTxt><p><span>Wände streichen</span></p></TextOutlTxt></OutlTxt></OutlineText>
          </CompleteText></Description>
        </Item></Itemlist>
      </BoQBody></BoQCtgy>
    </BoQBody></BoQ>
  </Award>
</GAEB>"#,
        )
        .unwrap();

        let document = read_gaeb_xml(&input).unwrap();
        assert_eq!(document.project, "Umbau Büro");
        assert_eq!(document.exchange_phase, "84");
        assert!(matches!(
            &document.rows[1],
            GaebRow::Item(item)
                if item.oz == "01.001"
                    && item.short_text == "Wände streichen"
                    && item.unit == "m²"
        ));

        write_gaeb_pdf(&document, &output).unwrap();
        assert!(fs::metadata(output).unwrap().len() > 1_000);
    }

    #[test]
    fn accepts_structured_x81_template_without_positions() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("template.x81");
        fs::write(
            &input,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<GAEB xmlns="http://www.gaeb.de/GAEB_DA_XML/DA81/3.3">
  <Award><DP>81</DP><BoQ><BoQBody>
    <BoQCtgy RNoPart="01"><LblTx><p><span>Vorlage</span></p></LblTx></BoQCtgy>
  </BoQBody></BoQ></Award>
</GAEB>"#,
        )
        .unwrap();
        let document = read_gaeb_xml(&input).unwrap();
        assert_eq!(document.exchange_phase, "81");
        assert!(matches!(document.rows[0], GaebRow::Category { .. }));
    }
}
