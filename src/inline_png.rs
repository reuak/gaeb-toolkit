use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use tempfile::tempdir;

use crate::model::{BillOfQuantities, Node, Position};

#[derive(Debug, Clone)]
struct InlinePng {
    page: usize,
    width: u32,
    data: String,
}

#[derive(Debug, Clone, Copy)]
struct PositionRef {
    page_from: usize,
    page_to: usize,
}

/// Extracts raster images from the source PDF and places each image in the
/// GAEB Item whose parsed PDF page range contains the image page.
///
/// The item order is identical to the X83 writer order. Images are inserted
/// into `DetailTxt/Text`; they are no longer collected in the global AddText.
pub fn inject_pdf_pngs(
    pdf_path: impl AsRef<Path>,
    x83_path: impl AsRef<Path>,
    boq: &BillOfQuantities,
) -> Result<usize> {
    let images = extract_pdf_pngs(pdf_path.as_ref())?;
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
        page_from: from,
        page_to: to,
    }
}

fn best_position_index(page: usize, positions: &[PositionRef]) -> Option<usize> {
    positions
        .iter()
        .enumerate()
        .filter(|(_, position)| position.page_from <= page && page <= position.page_to)
        .min_by_key(|(index, position)| {
            let span = position.page_to - position.page_from;
            // Engster Seitenbereich zuerst. Bei Gleichstand die Position mit
            // dem spätesten Beginn und anschließend die frühere Item-Reihenfolge.
            (span, usize::MAX - position.page_from, *index)
        })
        .map(|(index, _)| index)
}

fn extract_pdf_pngs(pdf_path: &Path) -> Result<Vec<InlinePng>> {
    let list = Command::new("pdfimages")
        .args(["-list", pdf_path.to_string_lossy().as_ref()])
        .output()
        .with_context(|| "pdfimages konnte nicht gestartet werden; Poppler vollständig installieren")?;

    if !list.status.success() {
        bail!(
            "pdfimages -list ist fehlgeschlagen: {}",
            String::from_utf8_lossy(&list.stderr).trim()
        );
    }

    let pages = parse_image_pages(&String::from_utf8_lossy(&list.stdout));
    if pages.is_empty() {
        return Ok(Vec::new());
    }

    let dir = tempdir()?;
    let mut images = Vec::new();

    for page in pages {
        let prefix = dir.path().join(format!("page-{page:04}-img"));
        let output = Command::new("pdfimages")
            .args([
                "-f",
                &page.to_string(),
                "-l",
                &page.to_string(),
                "-png",
                pdf_path.to_string_lossy().as_ref(),
                prefix.to_string_lossy().as_ref(),
            ])
            .output()
            .with_context(|| format!("PNG-Extraktion auf PDF-Seite {page} fehlgeschlagen"))?;

        if !output.status.success() {
            bail!(
                "PNG-Extraktion auf PDF-Seite {page} fehlgeschlagen: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let mut files = fs::read_dir(dir.path())?
            .filter_map(|entry| entry.ok().map(|value| value.path()))
            .filter(|path| is_page_png(path, page))
            .collect::<Vec<_>>();
        files.sort();

        for path in files {
            let bytes = fs::read(&path)?;
            let Some((width, height)) = png_dimensions(&bytes) else {
                continue;
            };
            if width < 32 || height < 32 {
                continue;
            }
            images.push(InlinePng {
                page,
                width,
                data: STANDARD.encode(bytes),
            });
        }
    }

    images.sort_by_key(|image| image.page);
    Ok(images)
}

fn parse_image_pages(output: &str) -> BTreeSet<usize> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let page = parts.next()?.parse::<usize>().ok()?;
            let _num = parts.next()?.parse::<usize>().ok()?;
            Some(page)
        })
        .collect()
}

fn is_page_png(path: &PathBuf, page: usize) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    name.starts_with(&format!("page-{page:04}-img-"))
        && path.extension().and_then(|value| value.to_str()) == Some("png")
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
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
        if let Some(index) = best_position_index(image.page, positions) {
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
    fn reads_png_dimensions() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&297u32.to_be_bytes());
        png.extend_from_slice(&224u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((297, 224)));
    }

    #[test]
    fn chooses_position_by_page_range() {
        let positions = [
            PositionRef { page_from: 10, page_to: 12 },
            PositionRef { page_from: 11, page_to: 11 },
        ];
        assert_eq!(best_position_index(11, &positions), Some(1));
    }

    #[test]
    fn injects_image_into_matching_item() {
        let xml = "<GAEB><Item ID=\"a\"><Description><CompleteText><OutlineText/></CompleteText></Description></Item><Item ID=\"b\"><Description><CompleteText><OutlineText/></CompleteText></Description></Item></GAEB>";
        let images = [InlinePng {
            page: 2,
            width: 297,
            data: "iVBORw0KGgo=".into(),
        }];
        let positions = [
            PositionRef { page_from: 1, page_to: 1 },
            PositionRef { page_from: 2, page_to: 2 },
        ];
        let (result, count) = inject_images_into_items(xml, &images, &positions).unwrap();
        assert_eq!(count, 1);
        let second = result.find("ID=\"b\"").unwrap();
        let image = result.find("<image width=\"297\"").unwrap();
        assert!(image > second);
    }
}
