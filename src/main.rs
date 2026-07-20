use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gaeb_toolkit::{
    export::{write_json, write_master_xml},
    parse_pdf, write_x83,
};

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
        /// GAEB DA XML 3.3 Angebotsaufforderung schreiben.
        #[arg(long)]
        x83: Option<PathBuf>,
        /// X83 trotz doppelter OZ oder unvollständiger Positionen schreiben.
        /// Nur nach manueller Prüfung verwenden.
        #[arg(long, requires = "x83")]
        allow_conflicts: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Parse {
            input,
            xml,
            json,
            x83,
            allow_conflicts,
        } => {
            let boq = parse_pdf(&input)?;
            if let Some(path) = xml {
                write_master_xml(&boq, path)?;
            }
            if let Some(path) = json {
                write_json(&boq, path)?;
            }
            if let Some(path) = x83 {
                write_x83(&boq, path, allow_conflicts)?;
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
