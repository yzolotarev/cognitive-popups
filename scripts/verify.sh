#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
python3 -m compileall -q src tests
PYTHONPATH=src python3 - <<'PY'
from cognitive_popups.models import CognitiveSession
from cognitive_popups.prompts import seed_prompt

session = CognitiveSession()
session.add_fragment("verification text", ["one", "two", "three", "four"])
assert len(session.fragments) == 1
assert len(session.fragments[0].cues) == 4
assert "verification text" in session.buffer_context()
assert len(session.clear().fragments) == 0
assert seed_prompt("x")[0]["role"] == "system"
print("cognitive-popups verification passed")
PY
