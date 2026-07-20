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
pub use x83::{write_x83, x83_conflicts};

pub fn parse_pdf(path: impl AsRef<Path>) -> anyhow::Result<BillOfQuantities> {
    let path = path.as_ref();
    let mut boq = parser::parse_pdf(path)?;
    placeholder_oz::recover_placeholder_positions(path, &mut boq)?;
    reference_cleanup::repair_split_references(&mut boq);
    pdf_cleanup::postprocess_pdf(path, &mut boq)?;
    Ok(boq)
}
