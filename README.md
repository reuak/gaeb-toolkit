# GAEB Toolkit

Rust-Werkzeug zum Extrahieren und Strukturieren deutscher Leistungsverzeichnisse aus PDF-Dateien.

## Funktionen

- erkennt die OZ-Hierarchie `AA`, `AA.BB`, `AA.BB.CC`, `AA.BB.CC.DDD`
- extrahiert Menge, Einheit, Einheitspreis und Gesamtbetrag
- übernimmt Kurztext, Langtext, Seitenbezug und Eventualpositionen
- prüft doppelte OZ und rechnerische Preisabweichungen
- exportiert Master-XML, JSON und GAEB DA XML 3.3 X83
- sperrt den X83-Export bei doppelten OZ oder unvollständigen Positionen bis zur manuellen Freigabe

## Voraussetzungen

- Rust Toolchain
- Poppler mit dem Programm `pdftotext`

macOS:

```bash
brew install poppler
```

Rust wird vorzugsweise über `rustup` installiert.

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

Master-XML und JSON:

```bash
cargo run --release -- parse angebot.pdf \
  --xml angebot.master.xml \
  --json angebot.json
```

GAEB-X83:

```bash
cargo run --release -- parse angebot.pdf --x83 angebot.x83
```

Bei Konflikten wird der X83-Export abgebrochen. Nach manueller Prüfung kann er ausdrücklich freigegeben werden:

```bash
cargo run --release -- parse angebot.pdf \
  --x83 angebot.x83 \
  --allow-conflicts
```

`--allow-conflicts` führt keine automatische Zusammenführung oder Korrektur durch. Doppelte Positionen bleiben getrennt erhalten.

Nach dem Release-Build liegt das Programm unter `target/release/gaeb-toolkit`.

## GAEB-Version

Der X83-Exporter schreibt GAEB DA XML 3.3, Datenphase 83, Versionsdatum `2021-05`. Die Ausgabe sollte zusätzlich mit dem GAEB-XML-Checker beziehungsweise einer geeigneten AVA-Software validiert werden.

## Aktueller Stand

Der Parser ist auf NOVA-ähnliche LV-Ausdrucke ausgerichtet. PDF ist ein Layoutformat; deshalb werden nicht eindeutig erkennbare oder rechnerisch auffällige Positionen als Warnungen ausgegeben.
