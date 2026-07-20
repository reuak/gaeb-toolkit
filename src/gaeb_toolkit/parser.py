from __future__ import annotations

import re
from decimal import Decimal, InvalidOperation
from pathlib import Path

import pdfplumber

from .model import BillOfQuantities, Node, Position

HEADING_RE = re.compile(r"^(?P<oz>\d{2}(?:\.\d{2}){0,2})\s+(?P<title>\S.*)$")
POSITION_RE = re.compile(
    r"^(?P<oz>\d{2}\.\d{2}\.\d{2}\.\d{3})\s+"
    r"(?P<qty>[\d.]+,\d{3})\s+(?P<unit>\S+)\s+"
    r"(?P<ep>[\d.]+,\d{2})\s*€"
    r"(?:\s+(?P<gb>[\d.]+,\d{2})\s*€|\s+Nur\s+Einh\.-Pr\.)?\s*$"
)
SUM_RE = re.compile(r"^Summe\s+(?P<oz>\d{2}(?:\.\d{2}){1,2})\b")
PAGE_FOOTER_RE = re.compile(r"^Druckausgabe vom:.*\d+\s*/\s*\d+\s*$")
REPEATED_HEADER_PREFIXES = (
    "Angebot",
    "Auftraggeber ",
    "Bieter ",
    "Projekt ",
    "LV ",
    "OZ Menge / Einheit EP GB",
)


def parse_decimal(value: str | None) -> Decimal | None:
    if not value:
        return None
    normalized = value.replace(".", "").replace(",", ".")
    try:
        return Decimal(normalized)
    except InvalidOperation:
        return None


def _is_noise(line: str) -> bool:
    return (
        not line
        or PAGE_FOOTER_RE.match(line) is not None
        or any(line.startswith(prefix) for prefix in REPEATED_HEADER_PREFIXES)
    )


def _extract_metadata(boq: BillOfQuantities, text: str) -> None:
    for line in text.splitlines():
        if line.startswith("Auftraggeber ") and not boq.client:
            boq.client = line.removeprefix("Auftraggeber ").strip()
        elif line.startswith("Bieter ") and not boq.bidder:
            boq.bidder = line.removeprefix("Bieter ").strip()
        elif line.startswith("Projekt ") and not boq.project:
            boq.project = line.removeprefix("Projekt ").strip()


def parse_pdf(path: str | Path) -> BillOfQuantities:
    source = Path(path)
    boq = BillOfQuantities(source=source.name)
    node_by_oz: dict[str, Node] = {}
    current_position: Position | None = None
    position_lines: list[str] = []
    preamble_lines: list[str] = []

    def finish_position(page_to: int | None = None) -> None:
        nonlocal current_position, position_lines
        if current_position is None:
            return
        cleaned = [line for line in position_lines if not _is_noise(line)]
        current_position.page_to = page_to or current_position.page_from
        if cleaned:
            current_position.short_text = cleaned[0]
            current_position.long_text = "\n".join(cleaned[1:]).strip()
        parent_oz = ".".join(current_position.oz.split(".")[:3])
        parent = node_by_oz.get(parent_oz)
        if parent is None:
            parent = _ensure_hierarchy(node_by_oz, boq, parent_oz, "", current_position.page_from)
            boq.warnings.append(f"Fehlender Untertitel für Position {current_position.oz} ergänzt.")
        parent.positions.append(current_position)
        current_position = None
        position_lines = []

    with pdfplumber.open(source) as pdf:
        for page_number, page in enumerate(pdf.pages, start=1):
            text = page.extract_text(x_tolerance=2, y_tolerance=3) or ""
            _extract_metadata(boq, text)
            for raw in text.splitlines():
                line = " ".join(raw.replace("\u00a0", " ").split()).strip()
                if _is_noise(line):
                    continue

                position_match = POSITION_RE.match(line)
                if position_match:
                    finish_position(page_number)
                    current_position = Position(
                        oz=position_match.group("oz"),
                        quantity=parse_decimal(position_match.group("qty")),
                        unit=position_match.group("unit"),
                        unit_price=parse_decimal(position_match.group("ep")),
                        total_price=parse_decimal(position_match.group("gb")),
                        page_from=page_number,
                        provisional="Nur Einh.-Pr." in line,
                        price_only="Nur Einh.-Pr." in line,
                    )
                    continue

                heading_match = HEADING_RE.match(line)
                if heading_match and len(heading_match.group("oz").split(".")) <= 3:
                    finish_position(page_number)
                    oz = heading_match.group("oz")
                    _ensure_hierarchy(
                        node_by_oz,
                        boq,
                        oz,
                        heading_match.group("title").strip(),
                        page_number,
                    )
                    continue

                if SUM_RE.match(line):
                    finish_position(page_number)
                    continue

                if current_position is not None:
                    if line == "Eventualposition ohne GB":
                        current_position.provisional = True
                        current_position.price_only = True
                    elif line not in {"Fortsetzung von vorheriger Seite", "Fortsetzung auf nächster Seite"}:
                        position_lines.append(line)
                elif page_number < 15:
                    preamble_lines.append(line)

    finish_position()
    boq.preamble = "\n".join(preamble_lines).strip()
    _validate(boq)
    return boq


def _ensure_hierarchy(
    node_by_oz: dict[str, Node],
    boq: BillOfQuantities,
    oz: str,
    title: str,
    page: int | None,
) -> Node:
    if oz in node_by_oz:
        node = node_by_oz[oz]
        if title and not node.title:
            node.title = title
        return node

    parts = oz.split(".")
    if len(parts) > 1:
        parent_oz = ".".join(parts[:-1])
        parent = _ensure_hierarchy(node_by_oz, boq, parent_oz, "", page)
    else:
        parent = None

    node = Node(oz=oz, title=title, level=len(parts), page=page)
    node_by_oz[oz] = node
    if parent is None:
        boq.roots.append(node)
    else:
        parent.children.append(node)
    return node


def _validate(boq: BillOfQuantities) -> None:
    seen: set[str] = set()

    def walk(node: Node) -> None:
        for position in node.positions:
            if position.oz in seen:
                boq.warnings.append(f"Doppelte OZ: {position.oz}")
            seen.add(position.oz)
            if position.quantity is None or position.unit_price is None:
                boq.warnings.append(f"Unvollständige Preiszeile: {position.oz}")
            if position.total_price is not None and position.quantity is not None and position.unit_price is not None:
                expected = position.quantity * position.unit_price
                if abs(expected - position.total_price) > Decimal("0.02"):
                    boq.warnings.append(
                        f"Preisabweichung {position.oz}: {expected} statt {position.total_price}"
                    )
        for child in node.children:
            walk(child)

    for root in boq.roots:
        walk(root)
