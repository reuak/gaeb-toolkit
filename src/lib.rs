use std::{collections::HashSet, path::Path, process::Command};

use anyhow::{bail, Context};

pub mod export;
pub mod inline_png;
pub mod model;
pub mod pdf_cleanup;
pub mod placeholder_oz;
pub mod price_cleanup;
pub mod priced_export;
pub mod reference_cleanup;
#[path = "parser_v2.rs"]
pub mod parser;
pub mod x83;

pub use inline_png::inject_pdf_pngs;
pub use model::{BillOfQuantities, Node, Position};
pub use parser::parse_text;
pub use priced_export::{write_x83_priced, write_x84};

pub fn parse_pdf(path: impl AsRef<Path>) -> anyhow::Result<BillOfQuantities> {
    let path = path.as_ref();

    // Der Layouttext wird vom Hauptparser und von der Platzhalter-OZ-Erkennung
    // benötigt. Früher wurde dieselbe PDF dafür zweimal mit pdftotext gelesen.
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

    let mut boq = parser::parse_text(source, &text)?;
    placeholder_oz::recover_placeholder_positions_from_text(&text, &mut boq)?;
    reference_cleanup::repair_split_references(&mut boq);
    pdf_cleanup::postprocess_pdf(path, &mut boq)?;
    price_cleanup::validate_and_repair_prices(&mut boq);
    Ok(boq)
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
