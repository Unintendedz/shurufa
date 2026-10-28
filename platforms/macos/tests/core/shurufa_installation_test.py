"""验证重复登记解析和备份封存，不修改系统登记或进程。"""
import importlib.util
import plistlib
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "installation", Path(__file__).parents[2] / "scripts/shurufa_installation.py")
installation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installation)


class InstallationTests(unittest.TestCase):
    def test_final_state_requires_only_installed_path_and_one_process(self):
        destination = Path("/synthetic/Input Methods/Shurufa.app")
        process = (123, destination / "Contents/MacOS/Shurufa")
        self.assertTrue(installation.installation_matches([destination], [process], destination))
        self.assertFalse(installation.installation_matches([destination, Path("/synthetic/build/Shurufa.app")], [process], destination))
        self.assertFalse(installation.installation_matches([destination], [process, process], destination))
        self.assertFalse(installation.installation_matches([destination], [], destination))

    def test_old_backups_move_outside_the_removable_state_root(self):
        with tempfile.TemporaryDirectory() as directory:
            support = Path(directory)
            legacy = support / "Shurufa/installation-backups"
            backup = support / "Shurufa Installation Backups"
            source = legacy / "synthetic-date/Shurufa.bundle-backup/SavedContents"
            source.mkdir(parents=True)
            (source / "Info.plist").write_bytes(b"synthetic preserved bytes")
            installation.migrate_backups(legacy, backup)
            saved = backup / "synthetic-date/Shurufa.bundle-backup/SavedContents/Info.plist"
            self.assertEqual(saved.read_bytes(), b"synthetic preserved bytes")
            self.assertFalse((legacy / "synthetic-date").exists())
            installation.migrate_backups(legacy, backup)
            source.mkdir(parents=True)
            (source / "Info.plist").write_bytes(b"synthetic second backup")
            with self.assertRaises(RuntimeError):
                installation.migrate_backups(legacy, backup)
            self.assertEqual(saved.read_bytes(), b"synthetic preserved bytes")
            self.assertEqual((source / "Info.plist").read_bytes(), b"synthetic second backup")

    def test_registered_paths_only_match_exact_identifier(self):
        separator = "\n" + "-" * 80 + "\n"
        dump = separator.join([
            "path: /synthetic/Input Methods/Shurufa.app (0x1234)\nidentifier: " + installation.IDENTIFIER,
            "path: /synthetic/build/Shurufa.app (0x22)\nidentifier: " + installation.IDENTIFIER,
            "path: /synthetic/Other.app (0x33)\nidentifier: " + installation.IDENTIFIER + ".Other",
        ])
        self.assertEqual(installation.registered_paths(dump), [
            Path("/synthetic/Input Methods/Shurufa.app"), Path("/synthetic/build/Shurufa.app")])

    def test_backup_preserves_all_files_and_can_be_restored(self):
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / "Shurufa.app"
            contents = bundle / "Contents"
            contents.mkdir(parents=True)
            payload = plistlib.dumps({"CFBundleIdentifier": installation.IDENTIFIER})
            (contents / "Info.plist").write_bytes(payload)
            saved = installation.preserve_bundle(bundle)
            self.assertFalse(bundle.exists())
            self.assertFalse((saved / "Contents").exists())
            self.assertEqual((saved / "SavedContents/Info.plist").read_bytes(), payload)
            (saved / "SavedContents").rename(saved / "Contents")
            saved.rename(bundle)
            self.assertEqual((contents / "Info.plist").read_bytes(), payload)


if __name__ == "__main__":
    unittest.main()
