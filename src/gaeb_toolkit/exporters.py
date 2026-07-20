from __future__ import annotations

import json
from pathlib import Path
from xml.etree.ElementTree import Element, ElementTree, SubElement, indent

from .model import BillOfQuantities, Node, Position


def write_json(boq: BillOfQuantities, path: str | Path) -> None:
    Path(path).write_text(
        json.dumps(boq.to_dict(), ensure_ascii=False, indent=2),
        encoding="utf-8",
    )


def write_master_xml(boq: BillOfQuantities, path: str | Path) -> None:
    root = Element("Leistungsverzeichnis", {"version": "0.1", "currency": boq.currency})
    meta = SubElement(root, "Metadaten")
    _text(meta, "Quelldatei", boq.source)
    _text(meta, "Projekt", boq.project)
    _text(meta, "Auftraggeber", boq.client)
    _text(meta, "Bieter", boq.bidder)

    if boq.preamble:
        _text(root, "Vorbemerkungen", boq.preamble)

    hierarchy = SubElement(root, "Hierarchie")
    for node in boq.roots:
        _write_node(hierarchy, node)

    if boq.warnings:
        validation = SubElement(root, "Validierung")
        for warning in boq.warnings:
            _text(validation, "Warnung", warning)

    indent(root, space="  ")
    ElementTree(root).write(path, encoding="utf-8", xml_declaration=True)


def _write_node(parent: Element, node: Node) -> None:
    names = {1: "Bereich", 2: "Titel", 3: "Untertitel"}
    element = SubElement(
        parent,
        names.get(node.level, "Gliederung"),
        {"oz": node.oz, "seite": str(node.page or "")},
    )
    _text(element, "Bezeichnung", node.title)
    for position in node.positions:
        _write_position(element, position)
    for child in node.children:
        _write_node(element, child)


def _write_position(parent: Element, position: Position) -> None:
    attrs = {
        "oz": position.oz,
        "seiteVon": str(position.page_from or ""),
        "seiteBis": str(position.page_to or ""),
        "eventual": str(position.provisional).lower(),
        "nurEinheitspreis": str(position.price_only).lower(),
    }
    element = SubElement(parent, "Position", attrs)
    _text(element, "Kurztext", position.short_text)
    _text(element, "Langtext", position.long_text)
    quantity = SubElement(element, "Menge", {"einheit": position.unit or ""})
    quantity.text = str(position.quantity or "")
    prices = SubElement(element, "Preise", {"waehrung": "EUR"})
    _text(prices, "Einheitspreis", str(position.unit_price or ""))
    _text(prices, "Gesamtbetrag", str(position.total_price or ""))


def _text(parent: Element, name: str, value: str) -> Element:
    element = SubElement(parent, name)
    element.text = value
    return element
