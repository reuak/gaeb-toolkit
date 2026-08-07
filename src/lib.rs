use std::{collections::HashSet, fs, path::Path, process::Command};

use anyhow::{bail, Context};

pub const CONVERTER_BRANDING: &str = "Umgewandelt mit GAEB.hawkvision.de";
pub const CONVERTER_NAME: &str = "GAEB.hawkvision.de";

mod breakdown;
pub mod export;
pub mod gaeb2000;
pub mod gaeb90;
pub mod gaeb_reader;
pub mod inline_png;
pub mod model;
#[path = "parser_v2.rs"]
pub mod parser;
pub mod pdf_cleanup;
pub mod placeholder_oz;
pub mod price_cleanup;
pub mod priced_export;
pub mod provisional_validation;
pub mod provisional_xml;
pub mod reference_cleanup;
pub mod title_cleanup;
pub mod x83;

pub use gaeb2000::read_gaeb_2000;
pub use gaeb90::{gaeb_document_to_boq, read_gaeb_90};
pub use gaeb_reader::{read_gaeb_xml, write_gaeb_pdf, GaebDocument, GaebItem, GaebRow};

pub fn read_gaeb(path: impl AsRef<Path>) -> anyhow::Result<GaebDocument> {
    let path = path.as_ref();
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("d81" | "d83") => read_gaeb_90(path),
        Some("p81" | "p83") => read_gaeb_2000(path),
        _ => read_gaeb_xml(path),
    }
}
pub use inline_png::inject_pdf_pngs;
pub use model::{BillOfQuantities, Node, Position};
pub use parser::parse_text;
pub use priced_export::{write_x83_priced, write_x84};
pub use provisional_xml::apply_provisional_flags;

pub fn parse_pdf(path: impl AsRef<Path>) -> anyhow::Result<BillOfQuantities> {
    let path = path.as_ref();

    // Der Layouttext wird vom Hauptparser, von der Platzhalter-OZ-Erkennung und
    // von der Summenprüfung gemeinsam genutzt. Es entsteht kein weiterer
    // pdftotext-/Poppler-Durchlauf.
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

    let extracted = String::from_utf8(output.stdout).context("PDF-Text ist nicht UTF-8")?;
    let (text, ocr_used) = if extracted.chars().filter(|c| !c.is_whitespace()).count() < 80 {
        (ocr_pdf_text(path)?, true)
    } else {
        (extracted, false)
    };
    let source = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("input.pdf");

    let mut boq = parser::parse_text(source, &text)?;
    if ocr_used {
        boq.warnings.push(
            "OCR-Vorprüfung verwendet: Das PDF enthielt keine ausreichende Textebene.".to_owned(),
        );
    }
    placeholder_oz::recover_placeholder_positions_from_text(&text, &mut boq)?;
    reference_cleanup::repair_split_references(&mut boq);
    pdf_cleanup::postprocess_pdf(path, &mut boq)?;
    title_cleanup::clean_titles(&mut boq.roots);
    price_cleanup::validate_and_repair_prices(&mut boq);
    provisional_validation::validate_provisional_totals(&text, &mut boq);
    Ok(boq)
}

fn ocr_pdf_text(path: &Path) -> anyhow::Result<String> {
    let directory = tempfile::tempdir()?;
    let prefix = directory.path().join("page");
    let rendered = Command::new("pdftoppm")
        .args([
            "-png",
            "-r",
            "200",
            path.to_string_lossy().as_ref(),
            prefix.to_string_lossy().as_ref(),
        ])
        .output()
        .context("PDF-Seiten konnten für OCR nicht gerendert werden")?;
    if !rendered.status.success() {
        bail!(
            "PDF-Seiten konnten für OCR nicht gerendert werden: {}",
            String::from_utf8_lossy(&rendered.stderr).trim()
        );
    }
    let mut pages = fs::read_dir(directory.path())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|entry| entry.extension().and_then(|v| v.to_str()) == Some("png"))
        .collect::<Vec<_>>();
    pages.sort();
    let mut text = String::new();
    for page in pages {
        let output = Command::new("tesseract")
            .arg(&page)
            .arg("stdout")
            .args([
                "-l",
                "deu+eng",
                "--psm",
                "6",
                "-c",
                "preserve_interword_spaces=1",
            ])
            .output()
            .context("Das PDF ist ein Scan, aber die deutsche OCR ist nicht installiert")?;
        if !output.status.success() {
            bail!(
                "Deutsche OCR ist fehlgeschlagen: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        text.push_str(&String::from_utf8_lossy(&output.stdout));
        text.push('\u{000C}');
    }
    // Nachtragsangebote verwenden häufig "NA 15.10" statt einer mindestens
    // dreiteiligen GAEB-OZ. Die synthetische Endstelle bleibt reproduzierbar.
    normalize_offer_oz(&text)
}

fn normalize_offer_oz(text: &str) -> anyhow::Result<String> {
    let offer_oz = regex::Regex::new(r"(?m)^\s*NA\s+(\d+)\.(\d+)\s+")?;
    Ok(offer_oz
        .replace_all(text, "$1.$2.001 NA $1.$2 ")
        .into_owned())
}

/// Liefert nur echte X83-Konflikte. Bei Positionen mit dem Vermerk
/// „Position entfällt“ sind Menge und Einheit nicht erforderlich.
pub fn x83_conflicts(boq: &BillOfQuantities) -> Vec<String> {
    // Die entfallenen OZ werden einmal gesammelt. Das vermeidet bei vielen
    // Konflikten eine wiederholte vollständige Traversierung des LV-Baums.
    let mut omitted = HashSet::new();
    collect_omitted_positions(&boq.roots, &mut omitted);

    x83::x83_conflicts(boq)
        .into_iter()
        .filter(|conflict| !is_omitted_quantity_or_unit_conflict(&omitted, conflict))
        .collect()
}

/// Schreibt die X83 und lässt fehlende Menge/Einheit ausschließlich bei
/// eindeutig als „Position entfällt“ gekennzeichneten Positionen zu.
pub fn write_x83(
    boq: &BillOfQuantities,
    path: impl AsRef<Path>,
    allow_conflicts: bool,
) -> anyhow::Result<()> {
    if allow_conflicts {
        return x83::write_x83(boq, path, true);
    }

    let conflicts = x83_conflicts(boq);
    if !conflicts.is_empty() {
        anyhow::bail!(
            "X83-Export gesperrt: {} Konflikt(e) müssen manuell geprüft werden:\n- {}\nDanach erneut mit --allow-conflicts exportieren.",
            conflicts.len(),
            conflicts.join("\n- ")
        );
    }

    // Der interne Writer kennt die Ausnahme „Position entfällt“ nicht. Nachdem
    // alle übrigen Konflikte gefiltert wurden, darf er den Export durchführen.
    x83::write_x83(boq, path, true)
}

fn is_omitted_quantity_or_unit_conflict(omitted: &HashSet<String>, conflict: &str) -> bool {
    let oz = conflict
        .strip_prefix("Menge fehlt: ")
        .or_else(|| conflict.strip_prefix("Einheit fehlt: "));
    oz.is_some_and(|value| omitted.contains(value.trim()))
}

fn collect_omitted_positions(nodes: &[Node], omitted: &mut HashSet<String>) {
    for node in nodes {
        for position in &node.positions {
            if is_omitted_position(position) {
                omitted.insert(position.oz.clone());
            }
        }
        collect_omitted_positions(&node.children, omitted);
    }
}

fn is_omitted_position(position: &Position) -> bool {
    position
        .short_text
        .lines()
        .chain(position.long_text.lines())
        .any(|line| line.trim().eq_ignore_ascii_case("Position entfällt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_short_na_number_for_gaeb_hierarchy() {
        let text = "NA 15.10 Anschluss GK-Wand\n1,000 m 8,25 8,25";
        assert_eq!(
            normalize_offer_oz(text).unwrap(),
            "15.10.001 NA 15.10 Anschluss GK-Wand\n1,000 m 8,25 8,25"
        );
    }

    #[test]
    fn omitted_position_needs_no_quantity_or_unit() {
        let mut boq = BillOfQuantities::new("test.pdf");
        boq.roots.push(Node {
            positions: vec![Position {
                oz: "01.01.01.130".into(),
                short_text: "Barken aufstellen".into(),
                long_text: "Position entfällt".into(),
                ..Position::default()
            }],
            ..Node::default()
        });

        assert!(x83_conflicts(&boq).is_empty());
    }

    #[test]
    fn normal_position_still_needs_quantity_and_unit() {
        let mut boq = BillOfQuantities::new("test.pdf");
        boq.roots.push(Node {
            positions: vec![Position {
                oz: "01.01.01.120".into(),
                short_text: "Barken aufstellen".into(),
                ..Position::default()
            }],
            ..Node::default()
        });

        let conflicts = x83_conflicts(&boq);
        assert!(conflicts.contains(&"Menge fehlt: 01.01.01.120".to_owned()));
        assert!(conflicts.contains(&"Einheit fehlt: 01.01.01.120".to_owned()));
    }
}
