use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use gaeb_toolkit::{
    apply_provisional_flags,
    export::{write_json, write_master_xml},
    inject_pdf_pngs, parse_pdf, write_x83, write_x83_priced, write_x84,
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
        /// GAEB DA XML 3.3 Angebotsaufforderung ohne Preise schreiben.
        #[arg(long)]
        x83: Option<PathBuf>,
        /// Zusätzliche X83 mit UP/IT schreiben. Nicht die reguläre GAEB-Angebotsabgabe.
        #[arg(long = "x83-priced")]
        x83_priced: Option<PathBuf>,
        /// GAEB DA XML 3.3 Angebotsabgabe mit EP und GP schreiben.
        #[arg(long)]
        x84: Option<PathBuf>,
        /// Exporte trotz verbleibender Konflikte schreiben.
        /// Nur nach manueller Prüfung verwenden.
        #[arg(long)]
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
            x83_priced,
            x84,
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
                write_x83(&boq, &path, allow_conflicts)?;
                embed_images(&input, &path, &boq, "X83")?;
                apply_provisional(&path, &boq, "X83")?;
            }

            match (x84, x83_priced) {
                (Some(x84_path), Some(x83_path)) => {
                    // Beide Preisformate haben denselben LV-Inhalt. Bilder werden
                    // nur einmal extrahiert; danach werden Namespace und DP für die
                    // zusätzliche X83 angepasst. Das spart einen pdftohtml-Lauf.
                    write_x84(&boq, &x84_path, allow_conflicts)?;
                    embed_images(&input, &x84_path, &boq, "X84")?;
                    apply_provisional(&x84_path, &boq, "X84")?;
                    derive_priced_x83(&x84_path, &x83_path)?;
                    eprintln!("X83 mit Preisen aus der X84 abgeleitet.");
                }
                (Some(path), None) => {
                    write_x84(&boq, &path, allow_conflicts)?;
                    embed_images(&input, &path, &boq, "X84")?;
                    apply_provisional(&path, &boq, "X84")?;
                }
                (None, Some(path)) => {
                    write_x83_priced(&boq, &path, allow_conflicts)?;
                    embed_images(&input, &path, &boq, "X83 mit Preisen")?;
                    apply_provisional(&path, &boq, "X83 mit Preisen")?;
                }
                (None, None) => {}
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

fn embed_images(
    input: &Path,
    output: &Path,
    boq: &gaeb_toolkit::BillOfQuantities,
    label: &str,
) -> Result<()> {
    let image_count = inject_pdf_pngs(input, output, boq)?;
    if image_count > 0 {
        eprintln!("{image_count} PNG-Abbildung(en) inline in die {label} eingebettet.");
    }
    Ok(())
}

fn apply_provisional(
    output: &Path,
    boq: &gaeb_toolkit::BillOfQuantities,
    label: &str,
) -> Result<()> {
    let count = apply_provisional_flags(output, boq)?;
    if count > 0 {
        eprintln!("{count} Eventualposition(en) in der {label} als 'WithoutTotal' markiert.");
    }
    Ok(())
}

fn derive_priced_x83(x84_path: &Path, x83_path: &Path) -> Result<()> {
    let source = fs::read_to_string(x84_path)
        .with_context(|| format!("X84 konnte nicht gelesen werden: {}", x84_path.display()))?;
    let converted = source
        .replace(
            "http://www.gaeb.de/GAEB_DA_XML/DA84/3.3",
            "http://www.gaeb.de/GAEB_DA_XML/DA83/3.3",
        )
        .replace("<DP>84</DP>", "<DP>83</DP>");
    fs::write(x83_path, converted)
        .with_context(|| format!("X83 mit Preisen konnte nicht geschrieben werden: {}", x83_path.display()))?;
    Ok(())
}
