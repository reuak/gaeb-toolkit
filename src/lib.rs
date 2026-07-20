use std::path::Path;

pub mod export;
pub mod inline_png;
pub mod model;
pub mod pdf_cleanup;
pub mod placeholder_oz;
pub mod reference_cleanup;
#[path = "parser_v2.rs"]
pub mod parser;
pub mod x83;

pub use inline_png::inject_pdf_pngs;
pub use model::{BillOfQuantities, Node, Position};
pub use parser::parse_text;

pub fn parse_pdf(path: impl AsRef<Path>) -> anyhow::Result<BillOfQuantities> {
    let path = path.as_ref();
    let mut boq = parser::parse_pdf(path)?;
    placeholder_oz::recover_placeholder_positions(path, &mut boq)?;
    reference_cleanup::repair_split_references(&mut boq);
    pdf_cleanup::postprocess_pdf(path, &mut boq)?;
    Ok(boq)
}

/// Liefert nur echte X83-Konflikte. Bei Positionen mit dem Vermerk
/// „Position entfällt“ sind Menge und Einheit nicht erforderlich.
pub fn x83_conflicts(boq: &BillOfQuantities) -> Vec<String> {
    x83::x83_conflicts(boq)
        .into_iter()
        .filter(|conflict| !is_omitted_quantity_or_unit_conflict(boq, conflict))
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

fn is_omitted_quantity_or_unit_conflict(boq: &BillOfQuantities, conflict: &str) -> bool {
    let oz = conflict
        .strip_prefix("Menge fehlt: ")
        .or_else(|| conflict.strip_prefix("Einheit fehlt: "));
    let Some(oz) = oz else {
        return false;
    };

    find_position(&boq.roots, oz.trim()).is_some_and(is_omitted_position)
}

fn find_position<'a>(nodes: &'a [Node], oz: &str) -> Option<&'a Position> {
    for node in nodes {
        if let Some(position) = node.positions.iter().find(|position| position.oz == oz) {
            return Some(position);
        }
        if let Some(position) = find_position(&node.children, oz) {
            return Some(position);
        }
    }
    None
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
