from __future__ import annotations

import argparse
from pathlib import Path

from .exporters import write_json, write_master_xml
from .parser import parse_pdf


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="gaeb-toolkit")
    subparsers = parser.add_subparsers(dest="command", required=True)

    parse_command = subparsers.add_parser("parse", help="LV-PDF analysieren")
    parse_command.add_argument("pdf", type=Path)
    parse_command.add_argument("--xml", type=Path, help="Master-XML-Ausgabe")
    parse_command.add_argument("--json", type=Path, help="JSON-Ausgabe")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "parse":
        boq = parse_pdf(args.pdf)
        xml_path = args.xml or args.pdf.with_suffix(".master.xml")
        write_master_xml(boq, xml_path)
        if args.json:
            write_json(boq, args.json)
        print(f"XML: {xml_path}")
        print(f"Warnungen: {len(boq.warnings)}")
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
