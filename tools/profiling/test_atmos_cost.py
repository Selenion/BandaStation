import copy
import unittest

from atmos_cost_report import gas_totals, process_window, workload_errors


class AtmosCostTests(unittest.TestCase):
    def test_complete_window_interpolates_cpu_and_integrates_memory(self):
        samples = [dict(utc=f"1970-01-01T00:00:0{i}+00:00", cpu_seconds=2*i,
                        main_thread_cpu_seconds=i, private_bytes=(1+2*i)*2**20,
                        working_set_bytes=(1+2*i)*2**20) for i in range(3)]
        result = process_window(samples, 0.5, 1.5)
        self.assertEqual(result["cpu_seconds"], 2)
        self.assertEqual(result["main_thread_cpu_seconds"], 1)
        self.assertEqual(result["private_bytes"], dict(mean_mib=3, peak_mib=4))
        with self.assertRaises(ValueError):
            process_window(samples, 0.5, 2.1)

    def test_workload_gate_rejects_different_fire_and_missing_jobs(self):
        row = dict(case="before", frames=1, tile_count=4, issued=1, worker_before=[0,0], worker_after=[0,1],
                   trace=[[[4,0,1], 8]], scenario="fire", state_hash="same", port_hash="same",
                   command_kinds=[0,8], commands=8, vents=0)
        other = copy.deepcopy(row)
        other["case"] = "current"
        self.assertFalse(workload_errors([row,other],1,2))
        other["trace"][0][0][1] = 10
        self.assertTrue(any("trace" in e["error"] for e in workload_errors([row,other],1,2)))
        other = copy.deepcopy(row)
        other["worker_after"][1] = 0
        self.assertTrue(any("job" in e["error"] for e in workload_errors([row,other],1,2)))

    def test_pipe_requires_real_plasma_and_direct_transfers(self):
        state=[1,1,100,100,1,2,6,1,293.15,2500,1,3,7,0]
        self.assertEqual(gas_totals(state),{3:7})
        row=dict(case="pipe",frames=1,tile_count=4,issued=1,worker_before=[0,0],worker_after=[0,1],
                 trace=[[[4,0,1],8]],scenario="pipe",state=state,vent_transfers=1)
        self.assertFalse(workload_errors([row],1,2))
        row["vent_transfers"]=0
        self.assertTrue(any("Direct" in e["error"] for e in workload_errors([row],1,2)))


if __name__ == "__main__":
    unittest.main()
