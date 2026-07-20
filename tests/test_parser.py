from decimal import Decimal

from gaeb_toolkit.model import BillOfQuantities
from gaeb_toolkit.parser import _ensure_hierarchy, parse_decimal


def test_parse_decimal_german_format() -> None:
    assert parse_decimal("1.263,50") == Decimal("1263.50")
    assert parse_decimal("361,000") == Decimal("361.000")


def test_hierarchy_is_built_from_oz() -> None:
    boq = BillOfQuantities(source="test.pdf")
    nodes = {}
    node = _ensure_hierarchy(nodes, boq, "01.01.02", "Schutzmaßnahmen", 19)

    assert node.oz == "01.01.02"
    assert boq.roots[0].oz == "01"
    assert boq.roots[0].children[0].oz == "01.01"
    assert boq.roots[0].children[0].children[0].title == "Schutzmaßnahmen"
