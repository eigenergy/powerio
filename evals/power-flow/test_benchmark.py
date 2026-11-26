import unittest
from unittest.mock import patch

from benchmark import compare, compare_admittance, compare_target_and_return, json_differences, report_changes
from solvers import ROOT, powsybl, powsybl_run


def solution(voltage, entries, *, base=100, branches=None):
    return {
        "status": "solved", "solver": "test solver", "units": "pu", "voltage": voltage,
        "one_island": True, "branch_power": branches or [],
        "admittance": {"basis": "network-pu", "labels": list(voltage), "entries": entries, "base_mva": base},
    }


class Comparisons(unittest.TestCase):
    def setUp(self):
        self.original = solution({"1": [1, 0], "2": [0.99, -1]}, [[0, 0, 2, 0], [1, 1, 1, 0]],
                                 branches=[["1", "2", 2, 1, -1.9, -0.9]])

    def test_matrix_norm_alignment_and_mva_base(self):
        target = solution({"2": [0.99, 4], "1": [1, 5]}, [[1, 1, 20, 0], [0, 0, 10, 0]], base=10,
                          branches=[["2", "1", -1.9, -0.9, 2, 1]])
        result = compare(self.original, target)
        self.assertEqual(result["status"], "agree")
        self.assertEqual(result["metrics"]["admittance_relative_2_norm"]["relative_error"], 0)

    def test_operator_norm_and_midway_error(self):
        converted = solution({"1": [1, 0], "2": [0.98, -2]}, [[0, 0, 2.2, 0], [1, 1, 1.2, 0]])
        direct = compare(self.original, converted)
        returned = compare(self.original, self.original)
        midway = compare_target_and_return({"direct": direct, "round_trip": returned}, converted, self.original)
        self.assertEqual(direct["status"], "different")
        self.assertEqual(returned["status"], "agree")
        self.assertEqual(midway["status"], "different")
        self.assertAlmostEqual(direct["metrics"]["admittance_relative_2_norm"]["relative_error"], 0.1)

    def test_missing_buses_and_parallel_branches_cannot_pass(self):
        wrong = solution({"1": [1, 0], "3": [0.99, -1]}, [[0, 0, 2, 0], [1, 1, 1, 0]])
        self.assertEqual(compare(self.original, wrong)["status"], "not_comparable")
        duplicate = {**self.original, "branch_power": self.original["branch_power"] * 2}
        self.assertEqual(compare(self.original, duplicate)["status"], "partial")

    def test_matches_converted_to_returned_through_original_labels(self):
        converted = solution({"A": [1, 0], "B": [0.99, -1]}, [[0, 0, 2, 0], [1, 1, 1, 0]],
                             branches=[["A", "B", 2, 1, -1.9, -0.9]])
        pair = {"direct": {"matched_labels": {"1": "A", "2": "B"}},
                "round_trip": {"matched_labels": {"1": "1", "2": "2"}}}
        result = compare_target_and_return(pair, converted, self.original)
        self.assertEqual(result["status"], "agree")
        self.assertEqual(result["matched_labels"], {"A": "1", "B": "2"})

    def test_zero_reference_admittance_has_no_relative_error(self):
        empty = solution({"1": [1, 0]}, [])
        self.assertEqual(compare_admittance(empty, empty, {"1": "1"})["status"], "not_comparable")

    def test_identifies_changed_fields_in_new_files(self):
        self.assertEqual(list(json_differences({"a/b": 1}, {"a/b": 2})), ["/a~1b"])
        self.assertEqual(list(json_differences({"bus": [{"gs": 1}]}, {"bus": [{"gs": 2}]})), ["/bus/0/gs"])

    def test_identifies_coverage_loss_separately_from_failed_checks(self):
        case = {"source": {"format": "matpower"}, "input": {"status": "ok"}}
        base = {"git_revision": "before", "tolerances": {}, "solvers": []}
        before = {"status": "agree", "nodes_compared": 14,
                  "metrics": {"voltage_pu": {"status": "agree", "maximum_difference": 0}}}
        after = {"status": "different", "nodes_compared": 12,
                 "metrics": {"voltage_pu": {"status": "different", "maximum_difference": 0.1}}}
        old, new = ({**base, "cases": {"case14": {**case, "conversions": {"xiidm": {"direct": result}}}}}
                    for result in (before, after))
        checks = {change["metric"]: change for change in report_changes(old, new)["changes"]}
        self.assertFalse(checks["overall"]["coverage_reduced"])
        self.assertTrue(checks["voltage_pu"]["coverage_reduced"])
        before["metrics"]["voltage_pu"] = {"status": "unavailable"}
        checks = {change["metric"]: change for change in report_changes(old, new)["changes"]}
        self.assertFalse(checks["voltage_pu"]["coverage_reduced"])

    def test_matching_a_subset_cannot_pass_the_whole_case(self):
        result = compare(self.original, self.original, pairing={"1": "1"})
        self.assertEqual(result["status"], "partial")
        self.assertEqual(result["unmatched_output_nodes"], ["2"])


class PowsyblAdmittance(unittest.TestCase):
    def test_recovers_when_one_island_dump_is_missing(self):
        def drop_one_dump(pp, network):
            results, models = powsybl_run(pp, network)
            return results, models[:1] if len(results) > 1 else models

        with patch("solvers.powsybl_run", side_effect=drop_one_dump):
            result = powsybl(ROOT / "tests/data/psse/case14_v35.raw")

        self.assertEqual(set(result["admittance"]["labels"]), {f"B{i}" for i in range(1, 15)})
        self.assertEqual(result["admittance"]["branch_stamps_checked_against_solved_flows"], 17)


if __name__ == "__main__":
    unittest.main()
