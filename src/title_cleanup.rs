use std::sync::LazyLock;

use regex::Regex;

use crate::model::Node;

static TRAILING_TOTAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(?:Summe\s+)?[\d.]+,\d{2}(?:\s*(?:€|EUR))?\s*$")
        .expect("valid trailing total regex")
});

/// Entfernt am Ende von Bereichs-, Titel- und Untertitelbezeichnungen
/// angehängte Summen aus dem PDF-Layout. Die Bereinigung erfolgt direkt am
/// Modell, damit sie für X83, bepreiste X83, X84, JSON und Master-XML gilt.
pub fn clean_titles(nodes: &mut [Node]) {
    for node in nodes {
        node.title = strip_trailing_totals(&node.title);
        clean_titles(&mut node.children);
    }
}

fn strip_trailing_totals(value: &str) -> String {
    let mut result = value.trim().to_owned();
    loop {
        let cleaned = TRAILING_TOTAL_RE.replace(&result, "").trim().to_owned();
        if cleaned == result {
            break;
        }
        result = cleaned;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_one_or_more_trailing_totals() {
        assert_eq!(
            strip_trailing_totals("Rückbauarbeiten 123.456,78 €"),
            "Rückbauarbeiten"
        );
        assert_eq!(
            strip_trailing_totals("Schutzmaßnahmen 1.000,00 EUR 2.000,00 €"),
            "Schutzmaßnahmen"
        );
        assert_eq!(strip_trailing_totals("Titel 2026"), "Titel 2026");
    }
}
