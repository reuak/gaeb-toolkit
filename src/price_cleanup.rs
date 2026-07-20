use rust_decimal::Decimal;

use crate::model::{BillOfQuantities, Node, Position};

const PRICE_TOLERANCE: Decimal = Decimal::new(2, 2);

/// Prüft Menge × EP gegen den Gesamtpreis und repariert eindeutige Parserfehler.
///
/// Regeln:
/// - vertauschte EP/GP werden erkannt und zurückgetauscht
/// - fehlender GP wird aus Menge × EP berechnet
/// - fehlender EP wird aus GP / Menge berechnet
/// - bei einer verbleibenden Abweichung gilt der explizite EP als maßgeblich;
///   der GP wird auf Menge × EP korrigiert und die Änderung protokolliert
pub fn validate_and_repair_prices(boq: &mut BillOfQuantities) {
    boq.warnings.retain(|warning| {
        !warning.starts_with("Preisabweichung ")
            && !warning.starts_with("Preis korrigiert: ")
            && !warning.starts_with("EP berechnet: ")
            && !warning.starts_with("GP berechnet: ")
    });

    let mut warnings = Vec::new();
    visit_nodes(&mut boq.roots, &mut warnings);
    boq.warnings.extend(warnings);
}

fn visit_nodes(nodes: &mut [Node], warnings: &mut Vec<String>) {
    for node in nodes {
        for position in &mut node.positions {
            repair_position(position, warnings);
        }
        visit_nodes(&mut node.children, warnings);
    }
}

fn repair_position(position: &mut Position, warnings: &mut Vec<String>) {
    if is_omitted(position) || position.price_only {
        return;
    }

    let Some(quantity) = position.quantity else {
        return;
    };
    if quantity.is_zero() {
        return;
    }

    match (position.unit_price, position.total_price) {
        (Some(unit_price), Some(total_price)) => {
            let expected = money(quantity * unit_price);
            if close(expected, total_price) {
                position.total_price = Some(expected);
                return;
            }

            // Typischer Spalten-/Zeilenfehler: EP und GP wurden vertauscht.
            let swapped_total = money(quantity * total_price);
            if close(swapped_total, unit_price) {
                position.unit_price = Some(total_price);
                position.total_price = Some(money(unit_price));
                warnings.push(format!(
                    "Preis korrigiert: {} – EP und GP waren wahrscheinlich vertauscht",
                    position.oz
                ));
                return;
            }

            // Der EP steht in einer eigenen, klar bezeichneten PDF-Spalte und ist
            // deshalb bei nicht eindeutig auflösbaren Abweichungen maßgeblich.
            position.total_price = Some(expected);
            warnings.push(format!(
                "Preis korrigiert: {} – GP {} wurde aus Menge × EP als {} neu berechnet",
                position.oz,
                decimal_text(total_price),
                decimal_text(expected)
            ));
        }
        (Some(unit_price), None) => {
            let total = money(quantity * unit_price);
            position.total_price = Some(total);
            warnings.push(format!(
                "GP berechnet: {} – {}",
                position.oz,
                decimal_text(total)
            ));
        }
        (None, Some(total_price)) => {
            let unit_price = money(total_price / quantity);
            position.unit_price = Some(unit_price);
            position.total_price = Some(money(quantity * unit_price));
            warnings.push(format!(
                "EP berechnet: {} – {}",
                position.oz,
                decimal_text(unit_price)
            ));
        }
        (None, None) => {}
    }
}

fn money(value: Decimal) -> Decimal {
    value.round_dp(2)
}

fn close(left: Decimal, right: Decimal) -> bool {
    (left - right).abs() <= PRICE_TOLERANCE
}

fn decimal_text(value: Decimal) -> String {
    value.round_dp(2).normalize().to_string()
}

fn is_omitted(position: &Position) -> bool {
    position
        .short_text
        .lines()
        .chain(position.long_text.lines())
        .any(|line| line.trim().eq_ignore_ascii_case("Position entfällt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(quantity: i64, ep: Option<Decimal>, gp: Option<Decimal>) -> Position {
        Position {
            oz: "01.01.01.010".into(),
            quantity: Some(Decimal::new(quantity, 0)),
            unit_price: ep,
            total_price: gp,
            ..Position::default()
        }
    }

    #[test]
    fn computes_missing_total() {
        let mut value = position(3, Some(Decimal::new(1250, 2)), None);
        repair_position(&mut value, &mut Vec::new());
        assert_eq!(value.total_price, Some(Decimal::new(3750, 2)));
    }

    #[test]
    fn swaps_obviously_reversed_prices() {
        let mut value = position(
            4,
            Some(Decimal::new(4000, 2)),
            Some(Decimal::new(1000, 2)),
        );
        repair_position(&mut value, &mut Vec::new());
        assert_eq!(value.unit_price, Some(Decimal::new(1000, 2)));
        assert_eq!(value.total_price, Some(Decimal::new(4000, 2)));
    }

    #[test]
    fn corrects_inconsistent_total_from_unit_price() {
        let mut value = position(
            2,
            Some(Decimal::new(1000, 2)),
            Some(Decimal::new(2500, 2)),
        );
        repair_position(&mut value, &mut Vec::new());
        assert_eq!(value.total_price, Some(Decimal::new(2000, 2)));
    }
}
