import importlib.util
import json
import pathlib
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name("setup-mac-workspace-mcp.py")
SPEC = importlib.util.spec_from_file_location("setup_mac_workspace_mcp", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class SetupTest(unittest.TestCase):
    def test_registration_preserves_other_connectors_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            config = root / "config.json"
            config.write_text(json.dumps({"other": {"kept": True}, "connectors": [{"id": "other"}]}))
            desired = MODULE.desired_connector(8083, root / "token")
            self.assertTrue(MODULE.register_connector(config, desired))
            self.assertFalse(MODULE.register_connector(config, desired))
            saved = json.loads(config.read_text())
            self.assertEqual(saved["other"], {"kept": True})
            self.assertEqual([item["id"] for item in saved["connectors"]], ["other", "dock-workspaces"])
            self.assertEqual(
                saved["connectors"][1]["capabilities"],
                [
                    "workspace.inspect",
                    "workspace.sync",
                    "workspace.present",
                    "canvas.present",
                    "presence.route",
                ],
            )

    def test_registration_refuses_foreign_collision(self):
        with tempfile.TemporaryDirectory() as directory:
            config = pathlib.Path(directory) / "config.json"
            config.write_text(json.dumps({"connectors": [{"id": "dock-workspaces", "metadata": {}}]}))
            with self.assertRaises(SystemExit):
                MODULE.register_connector(config, MODULE.desired_connector(8083, pathlib.Path(directory) / "token"))


if __name__ == "__main__":
    unittest.main()
