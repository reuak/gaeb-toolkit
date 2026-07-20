use std::collections::HashMap;

use regex::Regex;
use rust_decimal::Decimal;

use crate::model::{BillOfQuantities, Node, Position};

#[derive(Debug, Default, Clone, Copy)]
struct LayoutPriceFact {
    saw_price_row: bool,
    has_total_in_gb_column: bool,
    explicit_price_only: bool,
}

/// Prüft Eventualpositionen ohne GB mit bereits vorhandenem `pdftotext -layout`-Text.
/// Dadurch entsteht kein zusätzlicher PDF-/Poppler-Durchlauf.
///
/// Die Prüfung kombiniert zwei unabhängige Signale:
/// - Ist in der GB-Spalte der Positionszeile tatsächlich kein Gesamtpreis vorhanden?
/// - Entspricht die ausgewiesene Untertitelsumme der Summe ohne die Eventualposition?
///
/// Ergibt die Untertitelsumme dagegen, dass eine vermeintliche Eventualposition
/// enthalten sein muss, werden die Trigger entfernt und der GP aus Menge × EP gesetzt.
pub fn validate_provisional_totals(layout_text: &str, boq: &mut BillOfQuantities) {
    let (layout_facts, subtitle_totals) = extract_layout_facts(layout_text);
    let mut warnings = Vec::new();
    validate_nodes(
        &mut boq.roots,
        &layout_facts,
        &subtitle_totals,
        &mut warnings,
    );
    boq.warnings.extend(warnings);
}

fn extract_layout_facts(
    text: &str,
) -> (
    HashMap<String, LayoutPriceFact>,
    HashMap<String, Decimal>,
) {
    let position_re = Regex::new(r"^(?P<oz>\d{2}\.\d{2}\.\d{2}\.\d{3})(?:\s|$)")
        .expect("valid position regex");
    let price_re = Regex::new(r"[\d.]+,\d{3}\s+\S+\s+[\d.]+,\d{2}\s*€")
        .expect("valid price row regex");
    let money_re = Regex::new(r"[\d.]+,\d{2}\s*(?:€|EUR)").expect("valid money regex");
    let sum_re = Regex::new(r"^Summe\s+(?P<oz>\d{2}(?:\.\d{2}){1,2})\b")
        .expect("valid sum regex");

    let mut facts = HashMap::<String, LayoutPriceFact>::new();
    let mut totals = HashMap::<String, Decimal>::new();

    for page in text.split('\u{000C}') {
        let mut gb_column = None::<usize>;
        let mut current_oz = None::<String>;

        for raw in page.lines() {
            if raw.contains("OZ") && raw.contains("Menge / Einheit") && raw.contains("EP") {
                gb_column = raw.rfind("GB");
            }

            let normalized = normalize(raw);
            if normalized.is_empty() {
                continue;
            }

            if let Some(caps) = sum_re.captures(&normalized) {
                if let Some(total) = last_money(&normalized, &money_re) {
                    totals.insert(caps["oz"].to_owned(), total);
                }
                current_oz = None;
                continue;
            }

            if let Some(caps) = position_re.captures(&normalized) {
                current_oz = Some(caps["oz"].to_owned());
            }

            let Some(oz) = current_oz.as_ref() else {
                continue;
            };

            let explicit_price_only = normalized.contains("Nur Einh.-Pr.")
                || normalized.eq_ignore_ascii_case("Eventualposition ohne GB");
            let looks_like_price_row = price_re.is_match(&normalized) || explicit_price_only;
            if !looks_like_price_row {
                continue;
            }

            let fact = facts.entry(oz.clone()).or_default();
            fact.saw_price_row = true;
            fact.explicit_price_only |= explicit_price_only;

            if let Some(gb_start) = gb_column {
                fact.has_total_in_gb_column |= money_re
                    .find_iter(raw)
                    .any(|value| value.start() >= gb_start.saturating_sub(1));
            } else if price_re.is_match(&normalized) && !explicit_price_only {
                // Ohne erkennbare Kopfzeile ist ein zweiter Geldbetrag nach dem EP
                // das konservative Ersatzsignal für einen vorhandenen GB.
                fact.has_total_in_gb_column |= money_re.find_iter(&normalized).count() >= 2;
            }
        }
    }

    (facts, totals)
}

fn validate_nodes(
    nodes: &mut [Node],
    layout_facts: &HashMap<String, LayoutPriceFact>,
    subtitle_totals: &HashMap<String, Decimal>,
    warnings: &mut Vec<String>,
) {
    for node in nodes {
        if node.level == 3 {
            validate_subtitle(node, layout_facts, subtitle_totals, warnings);
        }
        validate_nodes(
            &mut node.children,
            layout_facts,
            subtitle_totals,
            warnings,
        );
    }
}

fn validate_subtitle(
    node: &mut Node,
    layout_facts: &HashMap<String, LayoutPriceFact>,
    subtitle_totals: &HashMap<String, Decimal>,
    warnings: &mut Vec<String>,
) {
    let candidates = node
        .positions
        .iter()
        .enumerate()
        .filter(|(_, position)| position.price_only || position.provisional)
        .filter_map(|(index, position)| expected_total(position).map(|total| (index, total)))
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        return;
    }

    let Some(reported) = subtitle_totals.get(&node.oz).copied() else {
        for (index, _) in candidates {
            let position = &node.positions[index];
            if layout_facts
                .get(&position.oz)
                .is_some_and(|fact| fact.has_total_in_gb_column)
            {
                warnings.push(format!(
                    "Eventualposition prüfen: {} – in der GB-Spalte wurde ein Gesamtpreis erkannt, aber keine Untertitelsumme gefunden",
                    position.oz
                ));
            }
        }
        return;
    };

    let base_total = money(
        node.positions
            .iter()
            .filter(|position| !position.price_only && !position.provisional)
            .filter_map(|position| position.total_price)
            .sum(),
    );
    let required = money(reported - base_total);

    if close(required, Decimal::ZERO) {
        for (index, _) in candidates {
            let position = &node.positions[index];
            if layout_facts
                .get(&position.oz)
                .is_some_and(|fact| fact.has_total_in_gb_column)
            {
                warnings.push(format!(
                    "Eventualposition bestätigt durch Untertitelsumme, aber GB-Spalte auffällig: {}",
                    position.oz
                ));
            }
        }
        return;
    }

    let selected = matching_subset(&candidates, required, &node.positions, layout_facts);
    let Some(selected) = selected else {
        warnings.push(format!(
            "Untertitelsumme prüfen: {} – ausgewiesen {}, aus regulären Positionen {}, Differenz {}",
            node.oz,
            decimal_text(reported),
            decimal_text(base_total),
            decimal_text(required)
        ));
        return;
    };

    for selected_index in selected {
        let (position_index, expected) = candidates[selected_index];
        let position = &mut node.positions[position_index];
        position.provisional = false;
        position.price_only = false;
        position.total_price = Some(expected);
        warnings.push(format!(
            "Eventualposition korrigiert: {} – Untertitelsumme enthält die Position; GP {} gesetzt",
            position.oz,
            decimal_text(expected)
        ));
    }
}

fn matching_subset(
    candidates: &[(usize, Decimal)],
    required: Decimal,
    positions: &[Position],
    layout_facts: &HashMap<String, LayoutPriceFact>,
) -> Option<Vec<usize>> {
    // In realen LVs gibt es nur sehr wenige Eventualpositionen je Untertitel.
    // Bis 16 Kandidaten ist die vollständige Teilmengenprüfung günstig und exakt.
    if candidates.len() <= 16 {
        let limit = 1usize << candidates.len();
        let mut best: Option<(usize, Vec<usize>)> = None;
        for mask in 1..limit {
            let mut sum = Decimal::ZERO;
            let mut selected = Vec::new();
            let mut layout_score = 0usize;
            for (candidate_index, (position_index, value)) in candidates.iter().enumerate() {
                if mask & (1usize << candidate_index) == 0 {
                    continue;
                }
                sum += *value;
                selected.push(candidate_index);
                let position = &positions[*position_index];
                if layout_facts
                    .get(&position.oz)
                    .is_some_and(|fact| fact.has_total_in_gb_column)
                {
                    layout_score += 1;
                }
            }
            if close(money(sum), required)
                && best
                    .as_ref()
                    .is_none_or(|(best_score, best_selected)| {
                        layout_score > *best_score
                            || (layout_score == *best_score
                                && selected.len() < best_selected.len())
                    })
            {
                best = Some((layout_score, selected));
            }
        }
        return best.map(|(_, selected)| selected);
    }

    // Sicherheitsfallback für ungewöhnlich viele Kandidaten: nur dann automatisch
    // korrigieren, wenn genau ein Einzelwert die Differenz erklärt.
    let singles = candidates
        .iter()
        .enumerate()
        .filter(|(_, (_, value))| close(*value, required))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    (singles.len() == 1).then(|| vec![singles[0]])
}

fn expected_total(position: &Position) -> Option<Decimal> {
    if let Some(total) = position.total_price {
        return Some(money(total));
    }
    Some(money(position.quantity? * position.unit_price?))
}

fn last_money(value: &str, money_re: &Regex) -> Option<Decimal> {
    let matched = money_re.find_iter(value).last()?.as_str();
    parse_decimal(
        matched
            .trim_end_matches("EUR")
            .trim_end_matches('€')
            .trim(),
    )
}

fn parse_decimal(value: &str) -> Option<Decimal> {
    value
        .replace('.', "")
        .replace(',', ".")
        .parse::<Decimal>()
        .ok()
}

fn normalize(value: &str) -> String {
    value
        .replace('\u{00A0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn money(value: Decimal) -> Decimal {
    value.round_dp(2)
}

fn close(left: Decimal, right: Decimal) -> bool {
    (left - right).abs() <= Decimal::new(2, 2)
}

fn decimal_text(value: Decimal) -> String {
    value.round_dp(2).normalize().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priced(oz: &str, total: i64) -> Position {
        Position {
            oz: oz.into(),
            quantity: Some(Decimal::ONE),
            unit: Some("St".into()),
            unit_price: Some(Decimal::new(total, 2)),
            total_price: Some(Decimal::new(total, 2)),
            ..Position::default()
        }
    }

    fn provisional(oz: &str, quantity: i64, ep: i64) -> Position {
        Position {
            oz: oz.into(),
            quantity: Some(Decimal::new(quantity, 0)),
            unit: Some("St".into()),
            unit_price: Some(Decimal::new(ep, 2)),
            provisional: true,
            price_only: true,
            ..Position::default()
        }
    }

    #[test]
    fn keeps_price_only_when_subtitle_sum_excludes_it() {
        let text = "OZ                    Menge / Einheit             EP              GB\n01.01.01.100          1,000 St                    10,00 €         10,00 €\n01.01.01.110          2,000 St                     5,00 € Nur Einh.-Pr.\nEventualposition ohne GB\nSumme 01.01.01 Test 10,00 €\n";
        let mut boq = BillOfQuantities::new("test.pdf");
        boq.roots.push(Node {
            oz: "01.01.01".into(),
            level: 3,
            positions: vec![priced("01.01.01.100", 1000), provisional("01.01.01.110", 2, 500)],
            ..Node::default()
        });

        validate_provisional_totals(text, &mut boq);
        assert!(boq.roots[0].positions[1].price_only);
        assert!(boq.warnings.is_empty());
    }

    #[test]
    fn restores_position_when_subtitle_sum_contains_it() {
        let text = "OZ                    Menge / Einheit             EP              GB\n01.01.01.100          1,000 St                    10,00 €         10,00 €\n01.01.01.110          2,000 St                     5,00 €         10,00 €\nEventualposition ohne GB\nSumme 01.01.01 Test 20,00 €\n";
        let mut boq = BillOfQuantities::new("test.pdf");
        boq.roots.push(Node {
            oz: "01.01.01".into(),
            level: 3,
            positions: vec![priced("01.01.01.100", 1000), provisional("01.01.01.110", 2, 500)],
            ..Node::default()
        });

        validate_provisional_totals(text, &mut boq);
        let restored = &boq.roots[0].positions[1];
        assert!(!restored.price_only);
        assert!(!restored.provisional);
        assert_eq!(restored.total_price, Some(Decimal::new(1000, 2)));
    }
}
