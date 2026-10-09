from __future__ import annotations

from pathlib import Path

import pytest

FIXTURES = Path(__file__).parent / "fixtures"
SAMPLES = {
    "mmdvm_20241124": FIXTURES / "mmdvm_20241124.F136",
    "mmdvm_20241127": FIXTURES / "mmdvm_20241127.F136",
    "dh98vu_20250811": FIXTURES / "dh98vu_20250811.F136",
}


@pytest.fixture(params=sorted(SAMPLES))
def sample_path(request: pytest.FixtureRequest) -> Path:
    return SAMPLES[request.param]


@pytest.fixture
def small_path() -> Path:
    return SAMPLES["mmdvm_20241127"]
