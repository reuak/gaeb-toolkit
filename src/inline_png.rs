use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{ImageFormat, ImageReader};
use quick_xml::escape::unescape;
use regex::Regex;
use tempfile::tempdir;

use crate::model::{BillOfQuantities, Node, Position};

#[derive(Debug, Clone)]
struct InlinePng {
    page: usize,
    top: i32,
    width: u32,
    data: String,
    target_oz: Option<String>,
}

#[derive(Debug, Clone)]
struct PositionRef {
    oz: String,
    page_from: usize,
    page_to: usize,
}

#[derive(Debug, Clone)]
struct TextMarker {
    top: i32,
    oz: String,
}

/// Extracts positioned PDF images with Poppler's `pdftohtml` and embeds each
/// image in the GAEB item belonging to the nearest preceding OZ on that page.
///
/// If a page does not repeat the OZ, the parser's page range is used as a
/// conservative fallback. No images are added to the global AddText block.
pub fn inject_pdf_pngs(
    pdf_path: impl AsRef<Path>,
    x83_path: impl AsRef<Path>,
    boq: &BillOfQuantities,
) -> Result<usize> {
    let images = extract_positioned_pngs(pdf_path.as_ref())?;
    if images.is_empty() {
        return Ok(0);
    }

    let positions = flatten_positions(boq);
    let x83_path = x83_path.as_ref();
    let xml = fs::read_to_string(x83_path)
        .with_context(|| format!("X83 konnte nicht gelesen werden: {}", x83_path.display()))?;
    let (updated, embedded) = inject_images_into_items(&xml, &images, &positions)?;
    fs::write(x83_path, updated)
        .with_context(|| format!("X83 konnte nicht geschrieben werden: {}", x83_path.display()))?;
    Ok(embedded)
}

fn flatten_positions(boq: &BillOfQuantities) -> Vec<PositionRef> {
    let mut result = Vec::new();
    for node in &boq.roots {
        flatten_node(node, &mut result);
    }
    result
}

fn flatten_node(node: &Node, result: &mut Vec<PositionRef>) {
    // Muss der Reihenfolge in x83::write_category entsprechen:
    // zuerst Untergliederungen, danach die Itemlist des aktuellen Knotens.
    for child in &node.children {
        flatten_node(child, result);
    }
    for position in &node.positions {
        result.push(position_ref(position));
    }
}

fn position_ref(position: &Position) -> PositionRef {
    let from = position.page_from.unwrap_or(1);
    let to = position.page_to.unwrap_or(from).max(from);
    PositionRef {
        oz: position.oz.clone(),
        page_from: from,
        page_to: to,
    }
}

fn best_position_index(image: &InlinePng, positions: &[PositionRef]) -> Option<usize> {
    if let Some(target_oz) = image.target_oz.as_deref() {
        if let Some(index) = positions
            .iter()
            .enumerate()
            .filter(|(_, position)| {
                position.oz == target_oz
                    && position.page_from <= image.page
                    && image.page <= position.page_to
            })
            .min_by_key(|(index, position)| {
                let span = position.page_to - position.page_from;
                (span, usize::MAX - position.page_from, *index)
            })
            .map(|(index, _)| index)
        {
            return Some(index);
        }

        if let Some(index) = positions
            .iter()
            .enumerate()
            .find(|(_, position)| position.oz == target_oz)
            .map(|(index, _)| index)
        {
            return Some(index);
        }
    }

    positions
        .iter()
        .enumerate()
        .filter(|(_, position)| {
            position.page_from <= image.page && image.page <= position.page_to
        })
        .min_by_key(|(index, position)| {
            let span = position.page_to - position.page_from;
            (span, usize::MAX - position.page_from, *index)
        })
        .map(|(index, _)| index)
}

fn extract_positioned_pngs(pdf_path: &Path) -> Result<Vec<InlinePng>> {
    let dir = tempdir()?;
    let xml_path = dir.path().join("layout.xml");
    let output = Command::new("pdftohtml")
        .args([
            "-xml",
            "-hidden",
            "-nodrm",
            "-enc",
            "UTF-8",
            pdf_path.to_string_lossy().as_ref(),
            xml_path.to_string_lossy().as_ref(),
        ])
        .output()
        .with_context(|| {
            "pdftohtml konnte nicht gestartet werden; Poppler vollständig installieren"
        })?;

    if !output.status.success() {
        bail!(
            "pdftohtml ist fehlgeschlagen: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let layout = fs::read_to_string(&xml_path)
        .with_context(|| format!("PDF-Layout konnte nicht gelesen werden: {}", xml_path.display()))?;
    parse_layout_images(&layout, dir.path())
}

fn parse_layout_images(layout: &str, base_dir: &Path) -> Result<Vec<InlinePng>> {
    let page_re = Regex::new(r#"(?s)<page\b(?P<attrs>[^>]*)>(?P<body>.*?)</page>"#)?;
    let text_re = Regex::new(r#"(?s)<text\b(?P<attrs>[^>]*)>(?P<body>.*?)</text>"#)?;
    let image_re = Regex::new(r#"<image\b(?P<attrs>[^>]*)/?>"#)?;
    let tag_re = Regex::new(r#"<[^>]+>"#)?;
    let oz_re = Regex::new(r#"\b\d{2}\.\d{2}\.\d{2}\.\d{3}\b"#)?;
    let mut images = Vec::new();

    for page_caps in page_re.captures_iter(layout) {
        let attrs = page_caps.name("attrs").map(|value| value.as_str()).unwrap_or("");
        let body = page_caps.name("body").map(|value| value.as_str()).unwrap_or("");
        let Some(page) = attr(attrs, "number").and_then(|value| value.parse::<usize>().ok()) else {
            continue;
        };

        let mut markers = Vec::new();
        for text_caps in text_re.captures_iter(body) {
            let text_attrs = text_caps.name("attrs").map(|value| value.as_str()).unwrap_or("");
            let top = attr(text_attrs, "top")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or_default();
            let raw = text_caps.name("body").map(|value| value.as_str()).unwrap_or("");
            let stripped = tag_re.replace_all(raw, "");
            let decoded = unescape(&stripped.replace("&nbsp;", " "))
                .map(|value| value.into_owned())
                .unwrap_or_else(|_| stripped.into_owned());
            if let Some(found) = oz_re.find(&decoded) {
                markers.push(TextMarker {
                    top,
                    oz: found.as_str().to_owned(),
                });
            }
        }
        markers.sort_by_key(|marker| marker.top);

        for image_caps in image_re.captures_iter(body) {
            let image_attrs = image_caps.name("attrs").map(|value| value.as_str()).unwrap_or("");
            let top = attr(image_attrs, "top")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or_default();
            let display_width = attr(image_attrs, "width")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(297);
            let display_height = attr(image_attrs, "height")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or_default();
            let Some(src) = attr(image_attrs, "src") else {
                continue;
            };
            let source_path = resolve_image_path(base_dir, &src);
            if !source_path.exists() {
                continue;
            }

            let reader = ImageReader::open(&source_path)
                .with_context(|| format!("Bild konnte nicht geöffnet werden: {}", source_path.display()))?
                .with_guessed_format()?;
            let decoded = reader
                .decode()
                .with_context(|| format!("Bild konnte nicht dekodiert werden: {}", source_path.display()))?;
            let source_width = decoded.width();
            let source_height = decoded.height();
            if source_width < 32 || source_height < 32 || display_width < 24 || display_height < 24 {
                continue;
            }

            let mut png = Cursor::new(Vec::new());
            decoded.write_to(&mut png, ImageFormat::Png)?;
            let target_oz = nearest_preceding_oz(top, &markers);
            images.push(InlinePng {
                page,
                top,
                width: display_width,
                data: STANDARD.encode(png.into_inner()),
                target_oz,
            });
        }
    }

    images.sort_by_key(|image| (image.page, image.top));
    Ok(images)
}

fn nearest_preceding_oz(top: i32, markers: &[TextMarker]) -> Option<String> {
    markers
        .iter()
        .filter(|marker| marker.top <= top)
        .max_by_key(|marker| marker.top)
        .or_else(|| markers.iter().min_by_key(|marker| (marker.top - top).abs()))
        .map(|marker| marker.oz.clone())
}

fn resolve_image_path(base_dir: &Path, src: &str) -> PathBuf {
    let path = Path::new(src);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base_dir.join(path)
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

fn image_xml(image: &InlinePng, indent: &str) -> String {
    format!(
        "{indent}<p>\n{indent}  <image width=\"{}\" Type=\"image/png\" Encoding=\"base64\">{}</image>\n{indent}</p>\n",
        image.width, image.data
    )
}

fn inject_images_into_items(
    xml: &str,
    images: &[InlinePng],
    positions: &[PositionRef],
) -> Result<(String, usize)> {
    let mut by_item = vec![Vec::<&InlinePng>::new(); positions.len()];
    for image in images {
        if let Some(index) = best_position_index(image, positions) {
            by_item[index].push(image);
        }
    }

    let mut result = String::with_capacity(xml.len() + images.len() * 1024);
    let mut cursor = 0usize;
    let mut item_index = 0usize;
    let mut embedded = 0usize;

    while let Some(relative_start) = xml[cursor..].find("<Item ") {
        let item_start = cursor + relative_start;
        result.push_str(&xml[cursor..item_start]);

        let Some(relative_end) = xml[item_start..].find("</Item>") else {
            bail!("Unvollständiges Item im X83-Dokument");
        };
        let item_end = item_start + relative_end + "</Item>".len();
        let item_xml = &xml[item_start..item_end];
        let assigned = by_item.get(item_index).map(Vec::as_slice).unwrap_or(&[]);
        result.push_str(&inject_into_item(item_xml, assigned));
        embedded += assigned.len();

        cursor = item_end;
        item_index += 1;
    }

    result.push_str(&xml[cursor..]);
    Ok((result, embedded))
}

fn inject_into_item(item_xml: &str, images: &[&InlinePng]) -> String {
    if images.is_empty() {
        return item_xml.to_owned();
    }

    let blocks = images
        .iter()
        .map(|image| image_xml(image, "            "))
        .collect::<String>();

    if let Some(index) = item_xml.find("</Text>") {
        let mut result = String::with_capacity(item_xml.len() + blocks.len());
        result.push_str(&item_xml[..index]);
        result.push_str(&blocks);
        result.push_str(&item_xml[index..]);
        return result;
    }

    if let Some(index) = item_xml.find("<OutlineText>") {
        let detail = format!(
            "        <DetailTxt>\n          <Text>\n{blocks}          </Text>\n        </DetailTxt>\n        "
        );
        let mut result = String::with_capacity(item_xml.len() + detail.len());
        result.push_str(&item_xml[..index]);
        result.push_str(&detail);
        result.push_str(&item_xml[index..]);
        return result;
    }

    item_xml.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_nearest_preceding_oz() {
        let markers = vec![
            TextMarker {
                top: 100,
                oz: "01.01.01.010".into(),
            },
            TextMarker {
                top: 500,
                oz: "01.01.01.020".into(),
            },
        ];
        assert_eq!(nearest_preceding_oz(450, &markers).as_deref(), Some("01.01.01.010"));
        assert_eq!(nearest_preceding_oz(700, &markers).as_deref(), Some("01.01.01.020"));
    }

    #[test]
    fn parses_positioned_image_from_layout() {
        let dir = tempdir().unwrap();
        let image_path = dir.path().join("layout-1_1.png");
        image::DynamicImage::new_rgb8(100, 80)
            .save_with_format(&image_path, ImageFormat::Png)
            .unwrap();
        let xml = r#"<pdf2xml><page number="1"><text top="100">01.01.01.010 Leistung</text><image top="220" width="297" height="100" src="layout-1_1.png"/></page></pdf2xml>"#;
        let images = parse_layout_images(xml, dir.path()).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].target_oz.as_deref(), Some("01.01.01.010"));
    }

    #[test]
    fn injects_image_into_matching_item() {
        let xml = "<GAEB><Item ID=\"a\"><Description><CompleteText><OutlineText/></CompleteText></Description></Item><Item ID=\"b\"><Description><CompleteText><OutlineText/></CompleteText></Description></Item></GAEB>";
        let images = [InlinePng {
            page: 2,
            top: 300,
            width: 297,
            data: "iVBORw0KGgo=".into(),
            target_oz: Some("01.01.01.020".into()),
        }];
        let positions = [
            PositionRef {
                oz: "01.01.01.010".into(),
                page_from: 1,
                page_to: 1,
            },
            PositionRef {
                oz: "01.01.01.020".into(),
                page_from: 2,
                page_to: 2,
            },
        ];
        let (result, count) = inject_images_into_items(xml, &images, &positions).unwrap();
        assert_eq!(count, 1);
        let second = result.find("ID=\"b\"").unwrap();
        let image = result.find("<image width=\"297\"").unwrap();
        assert!(image > second);
    }
}
