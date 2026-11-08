import copy
import unittest

from life_access_bench import cases, validate


class LifeAccessBenchTests(unittest.TestCase):
    def row(self):
        return dict(case="api", mode="api", frames=1, tile_count=4, issued=1,
                    worker_before=[0, 0], worker_after=[0, 1], trace=[[[4, 0, 1], 8]],
                    scenario="fire", state_hash="same", port_hash="same", command_kinds=[0, 8],
                    commands=8, vents=0, reading_trace=[[300, 7000]], breathing_trace=[[0, 0]],
                    life_trace=[[1, [[310, 315, 0, 0, 0, 0, 0]], 1]], breathers=1,
                    retained_references=0, references_enabled=True, mc_running=True,
                    gc_running=True, time_track_running=True, life_legacy=False, full_caches=0, background_fires={})

    def test_requires_environment_work_and_live_planner(self):
        row = self.row()
        self.assertEqual(validate([row], 1, 2), [])
        row["life_trace"][-1][2] = 0
        self.assertTrue(any("Life work" in e["error"] for e in validate([row], 1, 2)))
        row = self.row()
        row["gc_running"] = False
        self.assertTrue(any("disabled" in e["error"] for e in validate([row], 1, 2)))
        row = self.row()
        row["background_fires"] = {"objects": 1}
        self.assertTrue(any("background" in e["error"] for e in validate([row], 1, 2)))

    def test_rejects_changed_body_state_and_wrong_cache_path(self):
        row = self.row()
        legacy = copy.deepcopy(row)
        legacy.update(case="legacy", mode="legacy", life_legacy=True, full_caches=1)
        self.assertEqual(validate([legacy, row], 1, 2), [])
        row["life_trace"][0][1][0][0] = 320
        self.assertTrue(validate([legacy, row], 1, 2))
        row = self.row()
        row["full_caches"] = 1
        self.assertTrue(any("cache coverage" in e["error"] for e in validate([row], 1, 2)))

    def test_cases_keep_three_plain_repeats_and_separate_profiles(self):
        self.assertEqual(len(list(cases(False))), 16)
        self.assertEqual(len(list(cases(True))), 4)
        self.assertEqual(sum(c[3] == 0 for c in cases(False)), 12)


if __name__ == "__main__":
    unittest.main()
