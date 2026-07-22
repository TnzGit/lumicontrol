from __future__ import annotations

from pathlib import Path
import os
import subprocess


def main() -> int:
    root = Path(__file__).resolve().parent
    candidates = [
        root / "target" / "release" / "LumiControl.exe",
        root / "target" / "release" / "lumi-ui.exe",
    ]
    if local_app_data := os.environ.get("LOCALAPPDATA"):
        candidates.append(Path(local_app_data) / "LumiControl" / "LumiControl.exe")
    exe = next((candidate for candidate in candidates if candidate.is_file()), None)
    if exe is None:
        return 1

    creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    subprocess.Popen([str(exe)], cwd=str(exe.parent), creationflags=creationflags)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
