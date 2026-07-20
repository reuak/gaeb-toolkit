use std::collections::{HashMap, HashSet};

use crate::model::{BillOfQuantities, Node, Position};

/// Repariert PDF-Zeilen, in denen eine im Fließtext genannte OZ vom Rohparser
/// irrtümlich als neue Position erkannt wurde.
///
/// Eine echte Position benötigt grundsätzlich eine Preis-/Mengenzeile. Fehlen
/// Menge, Einheit und Preise vollständig, wird die vermeintliche Position nur
/// dann an den vorherigen Positionstext zurückgehängt, wenn der vorherige Text
/// einen Positionsverweis erkennen lässt. Summenzeilen und vollständig leere
/// Scheinpositionen werden entfernt. Ausnahmen wie „Position entfällt“ bleiben
/// als echte Position erhalten.
pub fn repair_split_references(boq: &mut BillOfQuantities) {
    for node in &mut boq.roots {
        repair_node(node);
    }
    refresh_parser_warnings(boq);
}

fn repair_node(node: &mut Node) {
    repair_positions(&mut node.positions);
    for child in &mut node.children {
        repair_node(child);
    }
}

fn repair_positions(positions: &mut Vec<Position>) {
    let mut repaired: Vec<Position> = Vec::with_capacity(positions.len());

    for candidate in positions.drain(..) {
        if is_real_position(&candidate) {
            repaired.push(candidate);
            continue;
        }

        let candidate_text = joined_text(&candidate);
        if is_sum_artifact(&candidate_text) || candidate_text.trim().is_empty() {
            continue;
        }

        if let Some(previous) = repaired.last_mut() {
            if is_reference_continuation(previous, &candidate) {
                append_reference_continuation(previous, &candidate);
                continue;
            }
        }

        // Unbepreiste Positionen, die keine sichere Fließtext-Fortsetzung sind,
        // bleiben zur manuellen Prüfung erhalten.
        repaired.push(candidate);
    }

    *positions = repaired;
}

fn is_real_position(position: &Position) -> bool {
    position.quantity.is_some()
        || position.unit.as_deref().is_some_and(|value| !value.trim().is_empty())
        || position.unit_price.is_some()
        || position.total_price.is_some()
        || position.provisional
        || position.price_only
        || is_omitted(position)
}

fn is_omitted(position: &Position) -> bool {
    joined_text(position)
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("Position entfällt"))
}

fn is_reference_continuation(previous: &Position, candidate: &Position) -> bool {
    if is_real_position(candidate) {
        return false;
    }

    let previous_text = joined_text(previous);
    let tail = previous_text
        .chars()
        .rev()
        .take(240)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .to_lowercase();

    let normalized_tail = tail.trim_end_matches(|c: char| c.is_whitespace());
    let explicit_reference = [
        "position ",
        "positionen ",
        "pos. ",
        "positionsbezug",
        "bezug zu position",
        "bezug auf position",
        "bereits in position",
        "in position",
    ]
    .iter()
    .any(|needle| normalized_tail.contains(needle));

    let open_conjunction = [" und", " sowie", " bzw.", ",", "/"]
        .iter()
        .any(|ending| normalized_tail.ends_with(ending));

    let nearby_pages = match (previous.page_to.or(previous.page_from), candidate.page_from) {
        (Some(previous_page), Some(candidate_page)) => candidate_page <= previous_page + 1,
        _ => true,
    };

    nearby_pages && (explicit_reference || open_conjunction)
}

fn append_reference_continuation(previous: &mut Position, candidate: &Position) {
    let mut continuation = candidate.oz.clone();
    let text = joined_text(candidate);
    if !text.trim().is_empty() {
        continuation.push(' ');
        continuation.push_str(text.trim());
    }

    if previous.long_text.trim().is_empty() {
        previous.long_text = continuation;
    } else {
        previous.long_text.push('\n');
        previous.long_text.push_str(&continuation);
    }

    if let Some(page_to) = candidate.page_to.or(candidate.page_from) {
        previous.page_to = Some(previous.page_to.unwrap_or(page_to).max(page_to));
    }
}

fn is_sum_artifact(value: &str) -> bool {
    let lower = value.trim().to_lowercase();
    lower.starts_with("summe")
        || lower.contains("untertitelsumme")
        || lower.contains("titel summe")
        || lower.contains("titelsumme")
        || lower.contains("summe untertitel")
        || lower.contains("summe titel")
}

fn joined_text(position: &Position) -> String {
    [position.short_text.trim(), position.long_text.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn refresh_parser_warnings(boq: &mut BillOfQuantities) {
    let mut counts = HashMap::<String, usize>::new();
    collect_counts(&boq.roots, &mut counts);
    let existing = counts.keys().cloned().collect::<HashSet<_>>();
    let duplicates = counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(oz, _)| oz.clone())
        .collect::<HashSet<_>>();

    boq.warnings.retain(|warning| {
        if let Some(oz) = warning.strip_prefix("Doppelte OZ: ") {
            return duplicates.contains(oz.trim());
        }
        if let Some(oz) = warning.strip_prefix("Unvollständige Preiszeile: ") {
            return existing.contains(oz.trim());
        }
        true
    });
}

fn collect_counts(nodes: &[Node], counts: &mut HashMap<String, usize>) {
    for node in nodes {
        for position in &node.positions {
            *counts.entry(position.oz.clone()).or_default() += 1;
        }
        collect_counts(&node.children, counts);
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;

    fn priced(oz: &str, long_text: &str) -> Position {
        Position {
            oz: oz.into(),
            quantity: Some(Decimal::ONE),
            unit: Some("St".into()),
            unit_price: Some(Decimal::ONE),
            total_price: Some(Decimal::ONE),
            short_text: "Leistung".into(),
            long_text: long_text.into(),
            page_from: Some(1),
            page_to: Some(1),
            ..Position::default()
        }
    }

    #[test]
    fn joins_oz_after_open_and_reference() {
        let mut positions = vec![
            priced(
                "02.05.01.130",
                "Ausführung gemäß den Positionen 02.05.01.100 und",
            ),
            Position {
                oz: "02.05.01.110".into(),
                short_text: "einschließlich Nebenarbeiten".into(),
                page_from: Some(1),
                page_to: Some(1),
                ..Position::default()
            },
        ];

        repair_positions(&mut positions);
        assert_eq!(positions.len(), 1);
        assert!(positions[0].long_text.contains("02.05.01.110 einschließlich Nebenarbeiten"));
    }

    #[test]
    fn joins_reference_in_sentence() {
        let mut positions = vec![
            priced(
                "02.02.01.060",
                "Die Ausführung ist bereits in Position",
            ),
            Position {
                oz: "02.02.01.050".into(),
                short_text: "beschrieben.".into(),
                ..Position::default()
            },
        ];

        repair_positions(&mut positions);
        assert_eq!(positions.len(), 1);
        assert!(positions[0].long_text.contains("02.02.01.050 beschrieben."));
    }

    #[test]
    fn keeps_unpriced_position_without_reference_context() {
        let mut positions = vec![
            priced("02.05.01.130", "Normaler Text."),
            Position {
                oz: "02.05.01.140".into(),
                short_text: "Ungeklärte unbepreiste Leistung".into(),
                ..Position::default()
            },
        ];

        repair_positions(&mut positions);
        assert_eq!(positions.len(), 2);
    }

    #[test]
    fn removes_subtitle_sum_artifact() {
        let mut positions = vec![
            priced("02.05.01.140", "Normaler Text."),
            Position {
                oz: "02.05.01.140".into(),
                short_text: "Summe Untertitel 02.05.01".into(),
                ..Position::default()
            },
        ];

        repair_positions(&mut positions);
        assert_eq!(positions.len(), 1);
    }

    #[test]
    fn keeps_omitted_position() {
        let mut positions = vec![Position {
            oz: "02.05.01.150".into(),
            short_text: "Leistung".into(),
            long_text: "Position entfällt".into(),
            ..Position::default()
        }];

        repair_positions(&mut positions);
        assert_eq!(positions.len(), 1);
    }
}
