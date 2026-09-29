"""Negative controls for the actual-source portability inventory."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

path = Path(__file__).with_name("check-command-structcopy-proof.py")
spec = importlib.util.spec_from_file_location("structcopy_proof", path)
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)


class StructcopyProofTests(unittest.TestCase):
    def fixture(self, root):
        leaf = root / proof.LEAF
        wrapper = root / proof.PROOF
        leaf.parent.mkdir(parents=True)
        wrapper.parent.mkdir(parents=True)
        leaf.write_text((proof.ROOT / proof.LEAF).read_text())
        wrapper.write_text((proof.ROOT / proof.PROOF).read_text())
        return leaf, wrapper

    def test_real_source_is_included(self):
        self.assertEqual(proof.violations(proof.ROOT), [])

    def test_stub_replacing_production_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _, wrapper = self.fixture(root)
            wrapper.write_text("pub mod structcopy {}")
            self.assertIn("actual structcopy", proof.violations(root)[0])

    def test_omitted_new_helper_requires_inventory_update(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            leaf, _ = self.fixture(root)
            leaf.write_text(leaf.read_text() + "\nmod hidden_host_helper;\n")
            self.assertIn("external helper", proof.violations(root)[0])
