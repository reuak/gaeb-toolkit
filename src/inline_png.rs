use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use tempfile::tempdir;

#[derive(Debug, Clone)]
struct InlinePng {
    page: usize,
    width: u32,
    data: String,
}

/// Extracts raster images from the source PDF with Poppler's `pdfimages`
/// and embeds them as Base64 PNGs in the first GAEB AddText block.
///
/// NOVA AVA writes inline images in this form:
/// `<p><image width="..." Type="image/png" Encoding="base64">...</image></p>`.
pub fn inject_pdf_pngs(pdf_path: impl AsRef<Path>, x83_path: impl AsRef<Path>) -> Result<usize> {
    let images = extract_pdf_pngs(pdf_path.as_ref())?;
    if images.is_empty() {
        return Ok(0);
    }

    let x83_path = x83_path.as_ref();
    let xml = fs::read_to_string(x83_path)
        .with_context(|| format!("X83 konnte nicht gelesen werden: {}", x83_path.display()))?;
    let updated = inject_images(&xml, &images)?;
    fs::write(x83_path, updated)
        .with_context(|| format!("X83 konnte nicht geschrieben werden: {}", x83_path.display()))?;
    Ok(images.len())
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
            // Kleine Masken, Linien und Symbole nicht als LV-Abbildung übernehmen.
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

fn inject_images(xml: &str, images: &[InlinePng]) -> Result<String> {
    let image_xml = images
        .iter()
        .map(|image| {
            format!(
                "\n          <p>\n            <image width=\"{}\" Type=\"image/png\" Encoding=\"base64\">{}</image>\n          </p>",
                image.width, image.data
            )
        })
        .collect::<String>();

    if let Some(index) = xml.find("</DetailAddText>") {
        let mut result = String::with_capacity(xml.len() + image_xml.len());
        result.push_str(&xml[..index]);
        result.push_str(&image_xml);
        result.push('\n');
        result.push_str(&xml[index..]);
        return Ok(result);
    }

    let Some(index) = xml.find("<BoQ ") else {
        bail!("X83 enthält weder DetailAddText noch BoQ-Einstiegspunkt");
    };
    let add_text = format!(
        "    <AddText>\n      <OutlineAddText>\n        <span>Abbildungen aus PDF</span>\n      </OutlineAddText>\n      <DetailAddText>{image_xml}\n      </DetailAddText>\n    </AddText>\n    "
    );
    let mut result = String::with_capacity(xml.len() + add_text.len());
    result.push_str(&xml[..index]);
    result.push_str(&add_text);
    result.push_str(&xml[index..]);
    Ok(result)
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
    fn injects_nova_inline_image_structure() {
        let xml = "<GAEB><Award><AddText><DetailAddText><p><span>Text</span></p></DetailAddText></AddText><BoQ ID=\"id1\"/></Award></GAEB>";
        let result = inject_images(
            xml,
            &[InlinePng {
                page: 1,
                width: 297,
                data: "iVBORw0KGgo=".into(),
            }],
        )
        .unwrap();
        assert!(result.contains("<image width=\"297\" Type=\"image/png\" Encoding=\"base64\">iVBORw0KGgo=</image>"));
    }
}
