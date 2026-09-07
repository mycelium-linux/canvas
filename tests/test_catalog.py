import copy
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from dock.catalog import InvalidConfig, compile_catalog, load


ROOT = Path(__file__).resolve().parents[1]


class CatalogTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        shutil.copytree(ROOT / 'profiles', self.root / 'profiles')
        shutil.copytree(ROOT / 'design', self.root / 'design')

    def change_profile(self, old, new):
        path = self.root / 'profiles/default.toml'
        path.write_text(path.read_text().replace(old, new))

    def change_tokens(self, mutate, update_hash=True):
        directory = self.root / 'design/coherence'
        path = directory / 'coherence-dark.tokens.json'
        tokens = json.loads(path.read_text())
        mutate(tokens)
        path.write_text(json.dumps(tokens))
        if update_hash:
            manifest_path = directory / 'manifest.json'
            manifest = json.loads(manifest_path.read_text())
            manifest['colorways']['dark']['sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
            manifest_path.write_text(json.dumps(manifest))

    def test_real_profiles_and_shared_colorways(self):
        catalog = load(self.root)
        self.assertEqual([p['id'] for p in catalog['profiles']], ['default', 'pedalkernel'])
        self.assertEqual(set(catalog['colorways']), {'dark', 'light'})
        self.assertNotEqual(catalog['colorways']['dark'], catalog['colorways']['light'])
        self.assertEqual(catalog['profiles'][1]['apps'][0]['cwd'], '~/src/fpl/pedalkernel')

    def test_deterministic_compilation_and_css_parity(self):
        output = self.root / 'out'
        catalog = compile_catalog(self.root, output)
        before = {p.name: p.read_bytes() for p in output.iterdir()}
        compile_catalog(self.root, output)
        self.assertEqual(before, {p.name: p.read_bytes() for p in output.iterdir()})
        self.assertEqual(json.loads((output / 'catalog.json').read_text()), catalog)
        for name, tokens in catalog['colorways'].items():
            css = (output / f'waybar-{name}.css').read_text()
            self.assertIn(f"@define-color dock_color_background {tokens['color-background']};", css)
            self.assertIn(f"padding: {tokens['space-2']} {tokens['space-3']}", css)

    def test_unknown_colorway_rejected_without_writing(self):
        self.change_profile('colorway = "dark"', 'colorway = "unknown"')
        output = self.root / 'out'
        with self.assertRaisesRegex(InvalidConfig, 'unknown colorway'):
            compile_catalog(self.root, output)
        self.assertFalse(output.exists())

    def test_duplicate_profile_rejected(self):
        shutil.copyfile(self.root / 'profiles/default.toml', self.root / 'profiles/duplicate.toml')
        with self.assertRaisesRegex(InvalidConfig, 'Duplicate'):
            load(self.root)

    def test_unknown_field_rejected(self):
        self.change_profile('session = "default"', 'session = "default"\nsesion = "typo"')
        with self.assertRaisesRegex(InvalidConfig, 'unknown fields'):
            load(self.root)

    def test_invalid_launch_fields_rejected(self):
        original = (self.root / 'profiles/default.toml').read_text()
        for old, new, message in [
            ('argv = ["foot"]', 'argv = "foot"', 'argv'),
            ('cwd = "~"', 'cwd = "relative/path"', 'cwd'),
            ('placement = "tiled"', 'placement = "banana"', 'placement'),
            ('platform = "linux"', 'platform = "unknown"', 'platform'),
        ]:
            with self.subTest(message=message):
                (self.root / 'profiles/default.toml').write_text(original.replace(old, new))
                with self.assertRaisesRegex(InvalidConfig, message):
                    load(self.root)

    def test_tokens_require_valid_colors_dimensions_and_semantic_types(self):
        original = json.loads((self.root / 'design/coherence/coherence-dark.tokens.json').read_text())
        cases = [('color-text', '$value', 'red; /* injected */'),
                 ('space-2', '$value', '-5px'),
                 ('color-text', '$type', 'dimension')]
        for key, field, value in cases:
            with self.subTest(key=key, field=field):
                def mutate(tokens):
                    tokens.clear()
                    tokens.update(copy.deepcopy(original))
                    tokens[key][field] = value
                self.change_tokens(mutate)
                with self.assertRaises(InvalidConfig):
                    load(self.root)

    def test_missing_semantic_token_rejected(self):
        self.change_tokens(lambda tokens: tokens.pop('color-background'))
        with self.assertRaisesRegex(InvalidConfig, 'missing required'):
            load(self.root)

    def test_provenance_tampering_rejected(self):
        self.change_tokens(lambda tokens: tokens['color-text'].update({'$value': '#000000'}), False)
        with self.assertRaisesRegex(InvalidConfig, 'checksum mismatch'):
            load(self.root)

    def test_manifest_path_escape_rejected(self):
        path = self.root / 'design/coherence/manifest.json'
        manifest = json.loads(path.read_text())
        manifest['colorways']['dark']['file'] = '../outside.tokens.json'
        path.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(InvalidConfig, 'local'):
            load(self.root)

    def test_invalid_input_leaves_existing_artifacts_untouched(self):
        output = self.root / 'out'
        compile_catalog(self.root, output)
        before = {p.name: p.read_bytes() for p in output.iterdir()}
        self.change_profile('workspace = "1:default"', 'workspace = ""')
        with self.assertRaises(InvalidConfig):
            compile_catalog(self.root, output)
        self.assertEqual(before, {p.name: p.read_bytes() for p in output.iterdir()})

    def test_cli_errors_are_actionable(self):
        result = subprocess.run([sys.executable, '-m', 'dock.cli', '--root', str(self.root / 'missing'), 'validate'],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('dock:', result.stderr)
        self.assertNotIn('Traceback', result.stderr)


if __name__ == '__main__':
    unittest.main()
