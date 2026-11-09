import copy
import unittest

from readings_bench import compare_observers, measurement_roundoff, validate
import test_life_access_bench as life_tests


class ReadingsBenchTests(unittest.TestCase):
    def row(self):
        row = life_tests.LifeAccessBenchTests().row()
        row.update(observer_count=1, scanned_ports=0, observer_trace=[[[0]]], port_owner_trace=[0])
        return row

    def test_common_life_and_original_observer_mode_are_independent(self):
        current = self.row()
        legacy = copy.deepcopy(current)
        legacy.update(case="legacy", mode="legacy", full_caches=1)
        self.assertEqual(validate([legacy, current], 1, 2), [])
        legacy["life_legacy"] = True
        self.assertTrue(any("common Life" in e["error"] for e in validate([legacy], 1, 2)))

    def test_rejects_changed_readings_and_unexpected_caches(self):
        current = self.row()
        legacy = copy.deepcopy(current)
        legacy.update(case="legacy", mode="legacy", full_caches=1)
        current["observer_trace"][0][0][0] = 2
        self.assertTrue(any("observer_trace" in e["error"] for e in validate([legacy, current], 1, 2)))
        current = self.row()
        current["full_caches"] = 1
        self.assertTrue(any("cache coverage" in e["error"] for e in validate([current], 1, 2)))

    def test_rejects_reading_that_disabled_a_native_port(self):
        current = self.row()
        current.update(vents=1, scanned_ports=1, observer_trace=[[[0], [0]]])
        self.assertTrue(any("port ownership" in e["error"] for e in validate([current], 1, 2)))
        current["port_owner_trace"] = [1]
        self.assertEqual(validate([current], 1, 2), [])

    def test_only_formatted_measurements_can_have_approved_numeric_roundoff(self):
        self.assertTrue(measurement_roundoff("7825.65 kPa", "7825.66 kPa"))
        self.assertFalse(measurement_roundoff("7825.65 kPa", "7825.66 mol"))
        self.assertFalse(measurement_roundoff("7825.65 kPa", "7830.00 kPa"))
        a = [[[0, "ok", [{"name": "Pressure", "value": "7825.65 kPa", "danger": 0}], [{"pressure": 7825.654, "gases": []}]]]]
        b = copy.deepcopy(a)
        b[0][0][2][0]["value"] = "7825.66 kPa"
        self.assertIsNone(compare_observers(a, b, 1)[0])
        self.assertEqual(len(compare_observers(a, b, 1)[1]), 1)
        b[0][0][2][0]["danger"] = 1
        self.assertIsNotNone(compare_observers(a, b, 1)[0])
        b = copy.deepcopy(a)
        b[0][0][3][0]["pressure"] = 7830
        self.assertIsNotNone(compare_observers(a, b, 1)[0])


if __name__ == "__main__":
    unittest.main()
