from __future__ import annotations

import sys
from pathlib import Path
from typing import Iterator

import pytest

# Run against the working tree without an install step, so `pytest` works
# straight after a checkout.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from bwbrowser import BwbrowserClient  # noqa: E402
from fake_bwbrowser import FakeBwbrowser  # noqa: E402

TOKEN = "test-token-abc123"


@pytest.fixture
def fake() -> Iterator[FakeBwbrowser]:
    server = FakeBwbrowser().start()
    try:
        yield server
    finally:
        server.stop()


@pytest.fixture
def client(fake: FakeBwbrowser) -> Iterator[BwbrowserClient]:
    with BwbrowserClient(token=TOKEN, port=fake.port, timeout=5.0, env={}) as connected:
        yield connected
