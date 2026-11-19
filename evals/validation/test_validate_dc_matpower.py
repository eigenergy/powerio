"""Keep the large-case oracle sparse without losing full-entry comparisons."""

import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
import scipy.sparse as sp
from matpowercaseframes import CaseFrames
from validate_dc_matpower import ABS_TOL, compare, matpower_dc, matrix_difference

ROOT = Path(__file__).resolve().parents[2]


class SparseMatrixComparisonTests(unittest.TestCase):
    def test_products_match_the_previous_dense_reference(self):
        reference = matpower_dc(CaseFrames(ROOT / "tests/data/case118.m"))
        incidence = reference["Cft"]
        weights = -reference["b"]
        dense_incidence = incidence.toarray()
        dense_branch = weights[:, None] * dense_incidence
        dense_bus = dense_incidence.T @ dense_branch
        sparse_branch = sp.diags(weights) @ incidence
        sparse_bus = incidence.T @ sparse_branch
        np.testing.assert_allclose(
            sparse_branch.toarray(), dense_branch, atol=ABS_TOL, rtol=0
        )
        np.testing.assert_allclose(
            sparse_bus.toarray(), dense_bus, atol=ABS_TOL, rtol=0
        )

    def test_difference_includes_implicit_zeros_and_changed_positions(self):
        left = sp.csr_matrix([[1.0, 0.0, -4.0], [0.0, 2.0, 0.0]])
        right = sp.csr_matrix([[0.0, 1.0, -4.0], [3.0, 0.0, 0.0]])
        expected = float(np.max(np.abs(left.toarray() - right.toarray())))
        self.assertEqual(matrix_difference(left, right), expected)
        self.assertEqual(matrix_difference(left, left), 0.0)

    def test_large_shape_does_not_allocate_dense_arrays(self):
        shape = (20_467, 13_659)
        left = sp.csr_matrix(([1.0], ([0], [0])), shape=shape)
        right = sp.csr_matrix(([1.0], ([shape[0] - 1], [shape[1] - 1])), shape=shape)
        with patch.object(
            sp.csr_matrix, "toarray", side_effect=AssertionError("dense allocation")
        ):
            self.assertEqual(matrix_difference(left, right), 1.0)
            self.assertEqual(matrix_difference(left, left), 0.0)
            self.assertEqual(
                compare(str(ROOT / "tests/data/case30.m"), want_ptdf=False), []
            )

    def test_nonfinite_differences_fail_the_tolerance_check(self):
        for value in (float("nan"), float("inf")):
            self.assertEqual(
                matrix_difference(sp.csr_matrix([[value]]), sp.csr_matrix((1, 1))),
                float("inf"),
            )


if __name__ == "__main__":
    unittest.main()
