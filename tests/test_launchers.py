from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_python_launcher_targets_v2_ui_executable():
    source = (ROOT / "launch_gui.pyw").read_text(encoding="utf-8")

    assert "brightness_controller" not in source
    assert "LumiControl.exe" in source
    assert "lumi-ui.exe" in source
    assert "screen-brightness.exe" not in source
    assert "CREATE_NO_WINDOW" in source


def test_vbs_launcher_targets_v2_ui_executable():
    source = (ROOT / "launch_rust_gui.vbs").read_text(encoding="utf-8")

    assert "target\\release\\LumiControl.exe" in source
    assert "target\\release\\lumi-ui.exe" in source
    assert "screen-brightness.exe" not in source
