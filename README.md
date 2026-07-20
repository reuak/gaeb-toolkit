# GAEB Toolkit

Rust-Werkzeug zum Extrahieren und Strukturieren deutscher Leistungsverzeichnisse aus PDF-Dateien.

## Funktionen

- erkennt die OZ-Hierarchie `AA`, `AA.BB`, `AA.BB.CC`, `AA.BB.CC.DDD`
- extrahiert Menge, Einheit, Einheitspreis und Gesamtbetrag
- übernimmt Kurztext, Langtext, Seitenbezug und Eventualpositionen
- prüft doppelte OZ und rechnerische Preisabweichungen
- exportiert eine Master-XML und JSON

## Voraussetzungen

- Rust Toolchain
- Poppler mit dem Programm `pdftotext`

macOS:

```bash
brew install rust poppler
```

Ubuntu/Debian:

```bash
sudo apt install cargo rustc poppler-utils
```

Windows: Rust über `rustup` installieren und Poppler in `PATH` aufnehmen.

## Bauen und testen

```bash
cargo build --release
cargo test
```

## Verwendung

```bash
cargo run --release -- parse angebot.pdf \
  --xml angebot.master.xml \
  --json angebot.json
```

Nach dem Release-Build liegt das Programm unter `target/release/gaeb-toolkit`.

## Aktueller Stand

Der Parser ist auf NOVA-ähnliche LV-Ausdrucke ausgerichtet. PDF ist ein Layoutformat; deshalb werden nicht eindeutig erkennbare oder rechnerisch auffällige Positionen als Warnungen ausgegeben. Der geplante GAEB-X83-Exporter wird auf dem strukturierten Datenmodell aufbauen.
