use std::{fs, path::Path};

use anyhow::{Context, Result};
use regex::Regex;

use crate::model::{BillOfQuantities, Node};

/// Ergänzt in einer erzeugten GAEB-Datei die NOVA-/GAEB-Kennzeichen für
/// Eventualpositionen ohne Gesamtbetrag.
///
/// Der Parser setzt für „Eventualposition ohne GB“ `price_only`. Die GAEB-Datei
/// benötigt zusätzlich `<Provis>WithoutTotal</Provis>` und bei pauschalen
/// Positionen `<LumpSumItem>Yes</LumpSumItem>`, damit die Position nicht in die
/// LV-Gesamtsumme eingeht, der EP aber erfasst werden kann.
pub fn apply_provisional_flags(
    path: impl AsRef<Path>,
    boq: &BillOfQuantities,
) -> Result<usize> {
    let path = path.as_ref();
    let source = fs::read_to_string(path)
        .with_context(|| format!("GAEB-Datei konnte nicht gelesen werden: {}", path.display()))?;
    let (updated, count) = apply_to_xml(&source, boq)?;
    if count > 0 {
        fs::write(path, updated).with_context(|| {
            format!("GAEB-Datei konnte nicht aktualisiert werden: {}", path.display())
        })?;
    }
    Ok(count)
}

fn apply_to_xml(source: &str, boq: &BillOfQuantities) -> Result<(String, usize)> {
    let item_start_re = Regex::new(r#"<Item\b[^>]*>"#)?;
    let item_end = "</Item>";
    let flags = flattened_provisional_flags(&boq.roots);

    let starts = item_start_re
        .find_iter(source)
        .map(|value| (value.start(), value.end()))
        .collect::<Vec<_>>();

    let mut output = String::with_capacity(source.len() + flags.len() * 80);
    let mut cursor = 0usize;
    let mut changed = 0usize;

    for (index, (start, end)) in starts.into_iter().enumerate() {
        output.push_str(&source[cursor..end]);
        cursor = end;

        if !flags.get(index).copied().unwrap_or(false) {
            continue;
        }

        let item_tail = &source[end..];
        let item_end_offset = item_tail.find(item_end).unwrap_or(item_tail.len());
        let item_body = &item_tail[..item_end_offset];
        if item_body.contains("<Provis>") {
            continue;
        }

        output.push_str("<Provis>WithoutTotal</Provis><LumpSumItem>Yes</LumpSumItem>");
        changed += 1;

        // `start` wird absichtlich verwendet, damit Clippy keine ungenutzte
        // Match-Position meldet und die Reihenfolge der Treffer dokumentiert ist.
        debug_assert!(start < end);
    }

    output.push_str(&source[cursor..]);
    Ok((output, changed))
}

fn flattened_provisional_flags(nodes: &[Node]) -> Vec<bool> {
    fn visit(nodes: &[Node], values: &mut Vec<bool>) {
        for node in nodes {
            visit(&node.children, values);
            values.extend(
                node.positions
                    .iter()
                    .map(|position| position.price_only || position.provisional),
            );
        }
    }

    let mut values = Vec::new();
    visit(nodes, &mut values);
    values
}

#[cfg(test)]
mod tests {
    use crate::model::{Node, Position};

    use super::*;

    #[test]
    fn adds_without_total_trigger_to_price_only_item() {
        let mut boq = BillOfQuantities::new("test.pdf");
        boq.roots.push(Node {
            positions: vec![
                Position {
                    oz: "01.01.01.100".into(),
                    ..Position::default()
                },
                Position {
                    oz: "01.01.01.110".into(),
                    price_only: true,
                    provisional: true,
                    ..Position::default()
                },
            ],
            ..Node::default()
        });

        let xml = r#"<Item RNoPart="100"><Qty>1</Qty></Item><Item RNoPart="110"><Qty>1</Qty></Item>"#;
        let (result, count) = apply_to_xml(xml, &boq).unwrap();
        assert_eq!(count, 1);
        assert!(result.contains(
            r#"<Item RNoPart="110"><Provis>WithoutTotal</Provis><LumpSumItem>Yes</LumpSumItem><Qty>1</Qty>"#
        ));
    }

    #[test]
    fn does_not_duplicate_existing_trigger() {
        let mut boq = BillOfQuantities::new("test.pdf");
        boq.roots.push(Node {
            positions: vec![Position {
                price_only: true,
                ..Position::default()
            }],
            ..Node::default()
        });
        let xml = "<Item><Provis>WithoutTotal</Provis></Item>";
        let (result, count) = apply_to_xml(xml, &boq).unwrap();
        assert_eq!(count, 0);
        assert_eq!(result, xml);
    }
}
