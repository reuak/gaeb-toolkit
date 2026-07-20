# GAEB Toolkit

Werkzeug zum Extrahieren und Strukturieren von Leistungsverzeichnissen aus PDF-Dateien.

## Aktueller Stand

Der erste Parser erkennt NOVA-ähnliche LV-PDFs mit folgender OZ-Hierarchie:

- `AA` – Bereich
- `AA.BB` – Titel
- `AA.BB.CC` – Untertitel
- `AA.BB.CC.DDD` – Position

Er extrahiert Text, Mengen, Einheiten, Einheitspreise, Gesamtbeträge, Eventualpositionen und Seitenreferenzen. Die Ausgabe erfolgt zunächst als strukturierte JSON- oder Master-XML-Datei. Ein GAEB-X83-Exporter ist als nächster Schritt vorgesehen.

## Installation

```bash
python -m venv .venv
source .venv/bin/activate  # Windows: .venv\Scripts\activate
pip install -e .
```

## Verwendung

```bash
gaeb-toolkit parse angebot.pdf --xml output.xml --json output.json
```

## Entwicklung

```bash
pip install -e ".[dev]"
pytest
```

## Hinweis

PDF-Dateien sind Layoutformate. Die Extraktion wird daher validiert und protokolliert; eine spätere GAEB-X83-Ausgabe sollte zusätzlich mit einem GAEB-Prüfwerkzeug geprüft werden.
