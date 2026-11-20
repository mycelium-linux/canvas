import importlib.util, json, pathlib, plistlib, tempfile, unittest
SCRIPT = pathlib.Path(__file__).with_name("setup-mac-canvas-mcp.py")
SPEC = importlib.util.spec_from_file_location("setup_mac_canvas_mcp", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC); SPEC.loader.exec_module(MODULE)

class SetupCanvasMcpTest(unittest.TestCase):
    def test_launch_agent_uses_visible_single_instance_app(self):
        with tempfile.TemporaryDirectory() as directory:
            home = pathlib.Path(directory)
            app = home / "Applications/DockCanvas.app"
            desired = plistlib.loads(MODULE.launch_agent(app, home))
            self.assertEqual(
                desired["ProgramArguments"],
                [str(app / "Contents/MacOS/dock-canvas")],
            )
            # A crash comes back; a clean quit (closing the window) does not.
            self.assertEqual(desired["KeepAlive"], {"SuccessfulExit": False})
            self.assertEqual(
                desired["EnvironmentVariables"]["CANVAS_CONFIG_DIR"],
                str(home / "Library/Application Support/Dock/canvas"),
            )

    def test_runtime_config_installs_binders_and_nested_named_views(self):
        with tempfile.TemporaryDirectory() as directory:
            support = pathlib.Path(directory) / "canvas"
            MODULE.install_runtime_config(support)
            self.assertTrue((support / "binders/widget-snapshot-v1.lua").is_file())
            self.assertTrue((support / "views/bifrost/runtime.json").is_file())
            self.assertTrue((support / "views/elmers/now.json").is_file())

    def test_registration_preserves_others_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "config.json"
            path.write_text(json.dumps({"connectors": [{"id": "other"}]}))
            desired = MODULE.desired_connector(8084, pathlib.Path("/absolute/token"))
            self.assertTrue(MODULE.register_connector(path, desired))
            saved = json.loads(path.read_text())
            self.assertEqual([item["id"] for item in saved["connectors"]], ["other", "dock-canvas"])
            # A stdio connector is never advertised to the control plane, so the
            # cloud route for dock.canvas-mcp only ever reached the bridge's
            # placeholder. An address the bridge can probe is the whole fix.
            self.assertEqual(saved["connectors"][1]["kind"], "mcp-streamable-http")
            self.assertEqual(saved["connectors"][1]["localUrl"], "http://127.0.0.1:8084/mcp")
            self.assertEqual(saved["connectors"][1]["localAuthTokenFile"], "/absolute/token")
            self.assertEqual(saved["connectors"][1]["metadata"]["capabilityId"], "dock.canvas-mcp")
            self.assertFalse(MODULE.register_connector(path, desired))

    def test_adapter_agent_restarts_so_the_capability_stays_advertised(self):
        with tempfile.TemporaryDirectory() as directory:
            home = pathlib.Path(directory)
            desired = plistlib.loads(
                MODULE.adapter_agent(home / ".local/bin/dock-canvas-mcp", home, 8084)
            )
            self.assertEqual(
                desired["ProgramArguments"],
                [str(home / ".local/bin/dock-canvas-mcp"), "http", "8084"],
            )
            # The bridge decides whether to advertise Canvas by probing this
            # address; an adapter that stays down after a crash un-publishes it.
            self.assertTrue(desired["KeepAlive"])

    def test_both_installers_agree_on_one_credential(self):
        # Two MCP servers, one file. When each installer minted its own, only the
        # one someone happened to run last was the one the servers agreed on.
        workspace = pathlib.Path(__file__).with_name("setup-mac-workspace-mcp.py")
        spec = importlib.util.spec_from_file_location("setup_mac_workspace_mcp", workspace)
        other = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(other)
        # Object identity would only prove each script ran the same file through
        # its own loader. What has to agree is the path they hand the servers.
        self.assertEqual(other.mcp_loopback.__file__, MODULE.mcp_loopback.__file__)
        home = pathlib.Path("/home/someone")
        self.assertEqual(
            other.mcp_loopback.token_path(home), MODULE.mcp_loopback.token_path(home)
        )
        # And the Rust servers read that same path; canvas::loopback pins its end.
        self.assertTrue(
            str(MODULE.mcp_loopback.token_path(home)).endswith(
                "Library/Application Support/Dock/mcp/token"
            )
        )

    def test_unmanaged_collision_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "config.json"
            path.write_text(json.dumps({"connectors": [{"id": "dock-canvas"}]}))
            with self.assertRaises(SystemExit):
                MODULE.register_connector(path, MODULE.desired_connector(8084, pathlib.Path("/t")))

if __name__ == "__main__": unittest.main()
