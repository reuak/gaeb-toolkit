use crate::model::{BillOfQuantities, Node};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Breakdown {
    pub level_lengths: Vec<usize>,
    pub item_length: usize,
}

pub(crate) fn from_boq(boq: &BillOfQuantities) -> Breakdown {
    let mut component_lengths = Vec::<usize>::new();
    collect_lengths(&boq.roots, &mut component_lengths);

    if component_lengths.is_empty() {
        return Breakdown {
            level_lengths: vec![2, 2, 2],
            item_length: 3,
        };
    }

    let item_length = component_lengths.pop().unwrap_or(3);
    Breakdown {
        level_lengths: component_lengths,
        item_length,
    }
}

pub(crate) fn level_label(index: usize) -> String {
    match index {
        0 => "Bereich".to_owned(),
        1 => "Titel".to_owned(),
        2 => "Untertitel".to_owned(),
        _ => format!("Gliederungsebene {}", index + 1),
    }
}

fn collect_lengths(nodes: &[Node], lengths: &mut Vec<usize>) {
    for node in nodes {
        for position in &node.positions {
            for (index, component) in position.oz.split('.').enumerate() {
                if index == lengths.len() {
                    lengths.push(component.len());
                } else {
                    lengths[index] = lengths[index].max(component.len());
                }
            }
        }
        collect_lengths(&node.children, lengths);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Position;

    #[test]
    fn derives_levels_and_lengths_from_positions() {
        let mut boq = BillOfQuantities::new("test.pdf");
        boq.roots.push(Node {
            positions: vec![
                Position {
                    oz: "1.2.10".into(),
                    ..Position::default()
                },
                Position {
                    oz: "12.3.100".into(),
                    ..Position::default()
                },
            ],
            ..Node::default()
        });

        assert_eq!(
            from_boq(&boq),
            Breakdown {
                level_lengths: vec![2, 1],
                item_length: 3,
            }
        );
    }
}
