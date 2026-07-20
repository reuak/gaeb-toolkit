use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gaeb_toolkit::{export::{write_json, write_master_xml}, parse_pdf};

#[derive(Debug, Parser)]
#[command(name = "gaeb-toolkit", version, about = "LV-PDFs strukturiert auslesen")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Parse {
        input: PathBuf,
        #[arg(long)]
        xml: Option<PathBuf>,
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Parse { input, xml, json } => {
            let boq = parse_pdf(&input)?;
            if let Some(path) = xml {
                write_master_xml(&boq, path)?;
            }
            if let Some(path) = json {
                write_json(&boq, path)?;
            }
            if boq.warnings.is_empty() {
                eprintln!("Parsing abgeschlossen.");
            } else {
                eprintln!("Parsing abgeschlossen mit {} Warnungen.", boq.warnings.len());
                for warning in &boq.warnings {
                    eprintln!("- {warning}");
                }
            }
        }
    }
    Ok(())
}
