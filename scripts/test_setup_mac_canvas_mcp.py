import importlib.util, json, pathlib, tempfile, unittest
SCRIPT = pathlib.Path(__file__).with_name("setup-mac-canvas-mcp.py")
SPEC = importlib.util.spec_from_file_location("setup_mac_canvas_mcp", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC); SPEC.loader.exec_module(MODULE)

class SetupCanvasMcpTest(unittest.TestCase):
    def test_registration_preserves_others_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "config.json"
            path.write_text(json.dumps({"connectors": [{"id": "other"}]}))
            desired = MODULE.desired_connector(pathlib.Path("/absolute/dock-canvas-mcp"))
            self.assertTrue(MODULE.register_connector(path, desired))
            saved = json.loads(path.read_text())
            self.assertEqual([item["id"] for item in saved["connectors"]], ["other", "dock-canvas"])
            self.assertEqual(saved["connectors"][1]["kind"], "mcp-stdio")
            self.assertFalse(MODULE.register_connector(path, desired))

    def test_unmanaged_collision_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "config.json"
            path.write_text(json.dumps({"connectors": [{"id": "dock-canvas"}]}))
            with self.assertRaises(SystemExit):
                MODULE.register_connector(path, MODULE.desired_connector(pathlib.Path("/bin/false")))

if __name__ == "__main__": unittest.main()
