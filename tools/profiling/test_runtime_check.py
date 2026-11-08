"""Focused tests for accepting reviewed runtimes without hiding new failures."""

from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from run import finish_capture, stage
from runtime_check import classify, validate_runtime_logs


PAINTING = """runtime error: The painting frame taking damage while having <= 0 integrity
proc name: take damage (/atom/proc/take_damage)
  source file: code/game/atom/atom_defense.dm,23
"""
AIR_ERROR = """runtime error: Cannot read null.volume
proc name: perform exposure (/obj/effect/hotspot/proc/perform_exposure)
  source file: code/modules/atmospherics/environmental/LINDA_fire.dm,206
"""
SM_ERROR = """runtime error: Cannot read null.z
proc name: is valid z level (/proc/is_valid_z_level)
  source file: code/__HELPERS/levels.dm,15
  call stack:
explosive delamination (/datum/sm_delam/explosive): effect demoralize(the supermatter crystal)
"""


class RuntimeCheckTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)

    def write_log(self, name, text):
        (self.folder / name).write_text(text, encoding="utf-8-sig")

    def finish(self, metadata=True, manifest=None):
        if metadata is True:
            metadata = {"scenario": "atmos-flood"}
        if metadata:
            (self.folder / "profile-metadata.json").write_text(json.dumps(metadata), encoding="utf-8")
        (self.folder / "tgstation.dmb").write_bytes(b"compiled fixture")
        with redirect_stdout(io.StringIO()):
            finish_capture(self.folder, {} if manifest is None else manifest, 60)
        return json.loads((self.folder / "manifest.json").read_text())

    def test_clean_capture_keeps_acceptance_metadata(self):
        result = self.finish()
        self.assertEqual(result["runtime_validation"]["status"], "clean")
        self.assertEqual(result["metadata"]["scenario"], "atmos-flood")
        self.assertTrue(result["binary_sha256"])

    def test_disabled_profile_accepts_only_the_pre_window_control_call(self):
        for profile in ([], [{"name": "/world/Profile", "calls": 1, "self": 0.001, "total": 0.001}]):
            with self.subTest(profile=profile):
                (self.folder / "profile-procs.json").write_text(json.dumps(profile), encoding="utf-8")
                result = self.finish(manifest={"proc_profiling": False})
                self.assertEqual(result["proc_profile_validation"]["unexpected"], [])

    def test_disabled_profile_rejects_gameplay_counts_or_recorded_time(self):
        for row in ({"name": "/datum/controller/subsystem/air/fire", "calls": 1},
                    {"name": "/datum/controller/subsystem/air/fire", "calls": 0, "real": 0.1},
                    {"name": "/world/Profile", "calls": 2}):
            with self.subTest(row=row):
                (self.folder / "profile-procs.json").write_text(json.dumps([row]), encoding="utf-8")
                with self.assertRaisesRegex(SystemExit, "remained enabled"):
                    self.finish(manifest={"proc_profiling": False})
                result = json.loads((self.folder / "manifest.json").read_text())
                self.assertEqual(result["proc_profile_validation"]["unexpected"], [row])

    def test_disabled_profile_requires_control_output(self):
        with self.assertRaisesRegex(SystemExit, "No proc-profile control"):
            self.finish(manifest={"proc_profiling": False})

    def test_requested_diagnostic_profile_cannot_ignore_forbid_config(self):
        metadata = {"scenario": "atmos-flood", "proc_profile_control": {"forbid_all_profiling": True}}
        with self.assertRaisesRegex(SystemExit, "forbidden by config"):
            self.finish(metadata, manifest={"proc_profiling": True})

    def test_staging_disabled_profile_does_not_change_source_config(self):
        source = self.folder / "source"
        (source / "config").mkdir(parents=True)
        settings = "#AUTO_PROFILE\nAUTO_PROFILE\n  auto_profile 1\nSERVERNAME test\n"
        (source / "config/config.txt").write_text(settings, encoding="utf-8")
        target = self.folder / "staged"
        with patch("run.ROOT", source):
            stage(target, proc_profile=False)
        self.assertEqual((source / "config/config.txt").read_text(), settings)
        actual = (target / "config/config.txt").read_text()
        self.assertIn("#AUTO_PROFILE", actual)
        self.assertIn("SERVERNAME test", actual)
        self.assertFalse(any(line.upper().split(maxsplit=1)[:1] == ["AUTO_PROFILE"] for line in actual.splitlines()))

    def test_decompression_requires_a_pressurized_chamber_and_pressure_drop(self):
        incident = {"inputs": {"pressure": 300}, "pressure_before": 300, "pressure_after": 225}
        metadata = {"scenario": "atmos-decompression", "incident": incident}
        self.assertEqual(self.finish(metadata)["metadata"], metadata)
        for before, after in ((100, 0), (300, 226), (300, None)):
            with self.subTest(before=before, after=after):
                incident.update(pressure_before=before, pressure_after=after)
                with self.assertRaisesRegex(SystemExit, "not pressurized and decompressed"):
                    self.finish(metadata)

    def test_plasma_flood_requires_plasma_at_half_the_outlets(self):
        incident = {"inputs": {"outlets": [[x, 1, 1] for x in range(1, 17)]}, "contaminated_outlets": 8}
        metadata = {"scenario": "pipe-plasma-flood", "incident": incident}
        self.assertEqual(self.finish(metadata)["metadata"], metadata)
        incident["contaminated_outlets"] = 7
        with self.assertRaisesRegex(SystemExit, "Fewer than half the outlets"):
            self.finish(metadata)

    def test_known_error_accepts_capture_and_preserves_counts(self):
        self.write_log("runtime.log.json", "\n".join(json.dumps({"msg": PAINTING}) for _ in range(2)))
        self.write_log("tgstation.log", PAINTING * 2)
        self.write_log("debug.log", PAINTING)
        report = self.finish()["runtime_validation"]
        self.assertEqual(report["status"], "accepted_with_known_runtimes")
        self.assertEqual(report["known_count"], 2)
        self.assertEqual(len(report["known"][0]["logs"]), 3)
        self.assertTrue(report["known"][0]["evidence"])

    def test_native_and_logis_mirrors_keep_signature_and_count(self):
        native = "[10:00:00] Runtime in code/__HELPERS/levels.dm,15: Cannot read null.z\n" + "\n".join(
            "  " + line for line in SM_ERROR.splitlines()[1:] if "source file:" not in line)
        logis = SM_ERROR.replace("\n", "\n - ")
        self.write_log("tgstation.log", native)
        self.write_log("logis.log", logis)
        self.write_log("runtime.log.json", json.dumps({"msg": SM_ERROR}))
        report = validate_runtime_logs(self.folder)
        self.assertEqual(report["known_count"], 1)
        self.assertEqual(report["unexpected_count"], 0)
        self.assertEqual(len(report["known"][0]["logs"]), 3)

    def test_known_error_does_not_hide_air_error(self):
        self.write_log("runtime.log.json", json.dumps({"msg": PAINTING + AIR_ERROR}))
        with self.assertRaisesRegex(SystemExit, "Unexpected runtime"):
            self.finish()
        manifest = json.loads((self.folder / "manifest.json").read_text())
        self.assertNotIn("metadata", manifest)
        self.assertEqual(manifest["runtime_validation"]["known_count"], 1)
        self.assertEqual(manifest["runtime_validation"]["unexpected_count"], 1)

    def test_same_file_or_proc_does_not_make_an_error_known(self):
        for text in (PAINTING.replace("painting frame", "water tank"),
                     PAINTING.replace("/atom/proc/take_damage", "/datum/banda_atmos_cell/proc/execute"),
                     PAINTING.replace("code/game/atom/atom_defense.dm", "modular_bandastation/atmos_rust/code/cell.dm"),
                     PAINTING.split("  source file:")[0]):
            with self.subTest(text=text):
                self.assertIsNone(classify(text))

    def test_generic_error_requires_reviewed_call_context(self):
        self.assertEqual(classify(SM_ERROR)["id"], "sm_demoralize_missing_location")
        self.assertIsNone(classify(SM_ERROR.split("  call stack:")[0]))
        self.assertIsNone(classify(SM_ERROR.replace("effect demoralize", "another callback")))

    def test_line_number_changes_do_not_hide_known_error(self):
        self.assertEqual(classify(PAINTING.replace(",23", ",41"))["id"], "painting_zero_integrity")

    def test_deleted_throw_requires_baseline_corpse_generation_context(self):
        error = """Runtime in code/game/atoms_movable.dm,1383: Qdeleted thing being thrown around.
  proc name: throw at (/atom/movable/proc/throw_at)
  call stack:
  /datum/corpse_damage/post_mort... (/datum/corpse_damage/post_mortem/organ_loss): apply to body(body)
"""
        self.assertEqual(classify(error)["id"], "corpse_organ_throw_after_delete")
        self.assertIsNone(classify(error.split("  call stack:")[0]))
        self.assertIsNone(classify(error.replace("/datum/corpse_damage/post_mortem/organ_loss", "/datum/banda_atmos_cell")))
        self.write_log("tgstation.log", error)
        report = self.finish()["runtime_validation"]
        self.assertEqual(report["known_count"], 1)
        self.assertEqual(report["unexpected_count"], 0)

    def test_unparsed_errors_and_infinite_loops_still_fail(self):
        for text in ("Runtime in unknown native callback", "Infinite loop suspected",
                     PAINTING + "Infinite loop suspected", '{"msg": "runtime error: broken record'):
            with self.subTest(text=text):
                self.write_log("runtime.log.json", text)
                self.assertGreater(validate_runtime_logs(self.folder)["unexpected_count"], 0)

    def test_missing_throw_target_requires_reviewed_tail_pin_context(self):
        cases = (("Cannot read null._listen_lookup", "pre_impact", "tail_pin_preimpact_missing_target"),
                 ("Cannot execute null.hitby().", "throw_impact", "tail_pin_impact_missing_target"))
        for error, proc, identifier in cases:
            with self.subTest(proc=proc):
                block = (f"Runtime in code/game/atoms_movable.dm,1360: {error}\n"
                         f"  proc name: impact (/atom/movable/proc/{proc})\n"
                         "  src: the tail pin (/obj/item/tail_pin)\n")
                self.assertEqual(classify(block)["id"], identifier)
                self.assertIsNone(classify(block.replace("/obj/item/tail_pin", "/obj/item/wrench")))
                self.assertIsNone(classify(block.split("  src:")[0]))
                self.write_log("tgstation.log", block)
                self.assertEqual(validate_runtime_logs(self.folder)["unexpected_count"], 0)

    def test_error_outside_json_message_still_fails(self):
        self.write_log("runtime.log.json", json.dumps({"msg": "status", "data": "Runtime in callback"}))
        self.assertEqual(validate_runtime_logs(self.folder)["unexpected_count"], 1)

    def test_known_error_does_not_accept_unfinished_capture(self):
        self.write_log("tgstation.log", PAINTING)
        with self.assertRaisesRegex(SystemExit, "No completed profile"):
            self.finish(metadata=False)
        self.assertFalse((self.folder / "manifest.json").exists())


if __name__ == "__main__":
    unittest.main()
