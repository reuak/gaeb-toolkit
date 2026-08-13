use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use gaeb_toolkit::{
    apply_provisional_flags,
    export::{write_json, write_master_xml},
    gaeb_document_to_boq, inject_pdf_pngs, parse_pdf, read_gaeb, write_d84, write_gaeb_pdf,
    write_p84, write_x83, write_x83_priced, write_x84,
};

#[derive(Debug, Parser)]
#[command(
    name = "gaeb-toolkit",
    version,
    about = "LV-PDFs strukturiert auslesen"
)]
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
    /// GAEB 90, GAEB DA 2000 oder GAEB DA XML lesen und neu ausgeben.
    ConvertGaeb {
        input: PathBuf,
        /// Lesbare PDF-Datei schreiben.
        #[arg(long)]
        pdf: Option<PathBuf>,
        /// Als modernes GAEB DA XML X83 schreiben.
        #[arg(long)]
        x83: Option<PathBuf>,
        /// Als GAEB DA 2000 P84-Angebotsabgabe schreiben (Preise erforderlich).
        #[arg(long)]
        p84: Option<PathBuf>,
        /// Als GAEB 90 D84-Angebotsabgabe schreiben (Preise erforderlich).
        #[arg(long)]
        d84: Option<PathBuf>,
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
                    // Die X84 bleibt als kompakter Angebotsrücklauf text- und bildfrei;
                    // die ausdrücklich gewünschte bepreiste X83 behält den LV-Inhalt.
                    write_x84(&boq, &x84_path, allow_conflicts)?;
                    apply_provisional(&x84_path, &boq, "X84")?;
                    write_x83_priced(&boq, &x83_path, allow_conflicts)?;
                    embed_images(&input, &x83_path, &boq, "X83 mit Preisen")?;
                    apply_provisional(&x83_path, &boq, "X83 mit Preisen")?;
                }
                (Some(path), None) => {
                    write_x84(&boq, &path, allow_conflicts)?;
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
                eprintln!(
                    "Parsing abgeschlossen mit {} Warnungen.",
                    boq.warnings.len()
                );
                for warning in &boq.warnings {
                    eprintln!("- {warning}");
                }
            }
        }
        Command::ConvertGaeb {
            input,
            pdf,
            x83,
            p84,
            d84,
        } => {
            let document = read_gaeb(&input)?;
            if pdf.is_none() && x83.is_none() && p84.is_none() && d84.is_none() {
                anyhow::bail!("Mindestens --pdf, --x83, --p84 oder --d84 angeben.");
            }
            if let Some(path) = pdf {
                write_gaeb_pdf(&document, path)?;
            }
            if let Some(path) = x83 {
                let boq = gaeb_document_to_boq(&document);
                write_x83(&boq, path, false)?;
            }
            if let Some(path) = p84 {
                write_p84(&document, path)?;
            }
            if let Some(path) = d84 {
                write_d84(&document, path)?;
            }
            eprintln!(
                "GAEB Phase {} mit {} Struktureinträgen gelesen.",
                document.exchange_phase,
                document.rows.len()
            );
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
