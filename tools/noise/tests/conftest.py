import datetime as dt
import random

import pytest

from hh_noise.model import Block, Cell, ColInfo
from hh_noise.sources import Column, Source, save_source

REGIONS = ["North", "South", "East", "West"]
CITIES = ["New York", "Los Angeles", "Chicago", "Houston", "Phoenix", "San Diego", "Dallas", "Austin"]


def make_source(n=120, seed=3) -> Source:
    rng = random.Random(seed)
    cols = [Column("Region", "category"), Column("City", "city"), Column("Customer Name", "name"), Column("Order Date", "date"),
            Column("Year", "year"), Column("Sales Amount", "money"), Column("Quantity", "count"), Column("Share", "percent"),
            Column("Paid", "bool"), Column("Profit", "money")]
    rows = []
    for i in range(n):
        d = dt.date(2023, 1 + (i // 12) % 12, 1 + i % 27)
        rows.append([REGIONS[(i // 9) % 4], rng.choice(CITIES), f"Customer {i:03d}", d, 2020 + i % 4,
                     round(rng.uniform(-500, 250000), 2), rng.randint(1, 900), round(rng.random(), 3),
                     rng.random() < 0.5, round(rng.uniform(-3000, 9000), 2)])
    return Source("fixture/sales.csv", "Sales Fixture", cols, rows, list(range(n)), n, [(0, n)])


@pytest.fixture
def source():
    return make_source()


@pytest.fixture
def sources_dir(tmp_path):
    d = tmp_path / "sources"
    d.mkdir()
    save_source(make_source(), d / "sales.json")
    s2 = make_source(150, seed=9)
    s2.name, s2.title = "fixture/other.csv", "Other Fixture"
    s2.columns = [Column("Segment", "category")] + s2.columns[1:]
    save_source(s2, d / "other.json")
    return d


@pytest.fixture
def block(source):
    from hh_noise.table import sample_block
    b = None
    rng = random.Random(1)
    while b is None:
        b = sample_block(source, rng, 30, 10, "en")
    return b


def tiny_block() -> Block:
    """Region | Amount | Date with 6 data rows (2 regions)."""
    cols = [ColInfo("Region", "category", 0, ["Region"]), ColInfo("Amount", "money", 1, ["Amount"]),
            ColInfo("Date", "date", 2, ["Date"])]
    cells = {(0, 0): Cell("Region", "header", (-1, 0), "Region"), (0, 1): Cell("Amount", "header", (-1, 1), "Amount"),
             (0, 2): Cell("Date", "header", (-1, 2), "Date")}
    data = [("North", 1200.5, dt.date(2024, 3, 5)), ("South", -30.0, dt.date(2024, 4, 15)), ("North", 99999.0, dt.date(2024, 3, 9)),
            ("South", 15.25, dt.date(2024, 4, 2)), ("North", 7.0, dt.date(2024, 5, 20)), ("South", 1234567.0, dt.date(2024, 5, 21))]
    for i, row in enumerate(data, start=1):
        for j, v in enumerate(row):
            cells[(i, j)] = Cell(v, "data", (i - 1, j), v)
    return Block("fixture/tiny.csv", "Tiny", cells, 7, 3, 1, cols, ["header"] + ["data"] * 6)
