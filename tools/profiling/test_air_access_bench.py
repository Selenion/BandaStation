import unittest

from air_access_bench import access_errors


class AirAccessBenchTests(unittest.TestCase):
    def test_breathing_gate_checks_actual_calls_mode_and_released_references(self):
        row = dict(case="api", mode="api", frames=1, tile_count=4, issued=1,
                   worker_before=[0, 0], worker_after=[0, 1], trace=[[[4, 0, 1], 8]],
                   scenario="fire", state_hash="same", port_hash="same", command_kinds=[0, 8],
                   commands=8, vents=0, reading_trace=[[300, 7000]], value_queries=1,
                   breathers=1, breathing_trace=[[1, 0.1]], retained_references=0, references_enabled=True)
        self.assertFalse(access_errors([row], 1, 2))
        row["retained_references"] = 1
        self.assertTrue(any("leaked" in e["error"] for e in access_errors([row], 1, 2)))
        row["retained_references"] = 0
        row["references_enabled"] = False
        self.assertTrue(any("access path" in e["error"] for e in access_errors([row], 1, 2)))

    def test_rejects_missing_observations_and_an_unused_native_path(self):
        row = dict(case="api", mode="api", frames=1, tile_count=4, issued=1,
                   worker_before=[0, 0], worker_after=[0, 1], trace=[[[4, 0, 1], 8]],
                   scenario="fire", state_hash="same", port_hash="same", command_kinds=[0, 8],
                   commands=8, vents=0, reading_trace=[[300, 7000]], value_queries=1)
        self.assertFalse(access_errors([row], 1, 2))
        row["value_queries"] = 0
        self.assertTrue(any("access path" in e["error"] for e in access_errors([row], 1, 2)))
        row["value_queries"] = 1
        row["reading_trace"] = []
        self.assertTrue(any("observations" in e["error"] for e in access_errors([row], 1, 2)))


if __name__ == "__main__":
    unittest.main()
