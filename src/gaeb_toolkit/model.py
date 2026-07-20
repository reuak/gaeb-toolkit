from __future__ import annotations

from dataclasses import asdict, dataclass, field
from decimal import Decimal
from typing import Any


@dataclass(slots=True)
class Position:
    oz: str
    quantity: Decimal | None = None
    unit: str | None = None
    unit_price: Decimal | None = None
    total_price: Decimal | None = None
    short_text: str = ""
    long_text: str = ""
    page_from: int | None = None
    page_to: int | None = None
    provisional: bool = False
    price_only: bool = False


@dataclass(slots=True)
class Node:
    oz: str
    title: str
    level: int
    page: int | None = None
    children: list[Node] = field(default_factory=list)
    positions: list[Position] = field(default_factory=list)


@dataclass(slots=True)
class BillOfQuantities:
    source: str
    project: str = ""
    client: str = ""
    bidder: str = ""
    currency: str = "EUR"
    preamble: str = ""
    roots: list[Node] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        def convert(value: Any) -> Any:
            if isinstance(value, Decimal):
                return str(value)
            if isinstance(value, list):
                return [convert(item) for item in value]
            if isinstance(value, dict):
                return {key: convert(item) for key, item in value.items()}
            return value

        return convert(asdict(self))
