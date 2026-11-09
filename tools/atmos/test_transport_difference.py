import unittest

from transport import difference


class TransportDifferenceTests(unittest.TestCase):
    def test_observer_labels_and_values_are_compared_without_numeric_coercion(self):
        before = {"gases": [["plasma", "Plasma", 2.0]], "temperature": 333.0}
        after = {"gases": [["plasma", "Plasma", 2.000001]], "temperature": 333.0}
        self.assertIsNone(difference(before, after))
        after["gases"][0][0] = "oxygen"
        self.assertIn("gases[0][0]", difference(before, after))
        # World references intentionally store high-precision pressure as numeric strings.
        self.assertIsNone(difference("561.610107", "561.609924"))
        self.assertIsNone(difference("1", 1))
        self.assertIsNotNone(difference({"name": "1"}, {"name": "1.000001"}, numeric_strings=False))
        self.assertIsNone(difference(None, None))


if __name__ == "__main__":
    unittest.main()
