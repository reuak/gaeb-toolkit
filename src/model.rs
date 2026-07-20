use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Position {
    pub oz: String,
    pub quantity: Option<Decimal>,
    pub unit: Option<String>,
    pub unit_price: Option<Decimal>,
    pub total_price: Option<Decimal>,
    pub short_text: String,
    pub long_text: String,
    pub page_from: Option<usize>,
    pub page_to: Option<usize>,
    pub provisional: bool,
    pub price_only: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub oz: String,
    pub title: String,
    pub level: usize,
    pub page: Option<usize>,
    pub children: Vec<Node>,
    pub positions: Vec<Position>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BillOfQuantities {
    pub source: String,
    pub project: String,
    pub client: String,
    pub bidder: String,
    pub currency: String,
    pub preamble: String,
    pub roots: Vec<Node>,
    pub warnings: Vec<String>,
}

impl BillOfQuantities {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            project: String::new(),
            client: String::new(),
            bidder: String::new(),
            currency: "EUR".to_owned(),
            preamble: String::new(),
            roots: Vec::new(),
            warnings: Vec::new(),
        }
    }
}
