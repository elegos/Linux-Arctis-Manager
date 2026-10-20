"""sink-visibility.lua must load on both WirePlumber logging APIs."""
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "packaging" / "wireplumber" / "sink-visibility.lua"
HARNESS = Path(__file__).with_name("harness.lua")

LUA = shutil.which("lua") or shutil.which("lua5.4") or shutil.which("luajit")
pytestmark = pytest.mark.skipif(LUA is None, reason="lua interpreter not installed")


def run(variant: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [LUA, str(HARNESS), str(SCRIPT), variant],
        capture_output=True, text=True,
    )


@pytest.mark.parametrize("variant", ["topic", "legacy"])
def test_script_loads_and_logs(variant):
    result = run(variant)
    assert result.returncode == 0, result.stderr
    # file-monitor-api is stubbed missing, so the script logs a warning at load.
    warnings = [l for l in result.stdout.splitlines() if l.startswith("warning|")]
    assert any("file-monitor-api plugin not available" in l for l in warnings)
