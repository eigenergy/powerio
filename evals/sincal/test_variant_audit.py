"""Original structural cases, including the Siemens 6.0 documented branch shape."""
import unittest

from audit_variants import active_rows, ancestry, compare, index_tables, resolve


def variant(identifier, parent=None, active=0):
    return dict(Variant_ID=identifier, ParentVariant_ID=parent, Flag_Variant=active)


def node(identifier, owner, active=0, **fields):
    return dict(Node_ID=identifier, Variant_ID=owner, Flag_Variant=active, **fields)


class VariantAuditTests(unittest.TestCase):
    def tables(self):
        return {'Variant': [variant(1), variant(2, 1), variant(3, 2, 1), variant(4, 1)],
                'Node': [node(1, 1, A='base', B='unchanged', C='base'),
                         node(1, 3, 1, A='child', B='unchanged', C=None),
                         node(1, 4, A='base', B='unchanged', C='sibling'),
                         node(2, 4, A='new', B='new', C='new')]}

    def test_documented_branch_shape_preserves_null_override_and_excludes_sibling(self):
        tables = self.tables()
        variants, indexed = index_tables(tables)
        self.assertEqual(ancestry(variants, 3), [1, 2, 3])
        result = resolve(indexed, ancestry(variants, 3))
        self.assertEqual(result['Node'], {1: tables['Node'][1]})
        self.assertIsNone(result['Node'][1]['C'])
        self.assertEqual(compare(result, active_rows(indexed)), 0)
        self.assertEqual(set(resolve(indexed, ancestry(variants, 4))['Node']), {1, 2})
        self.assertEqual(resolve(indexed, ancestry(variants, 2))['Node'][1]['A'], 'base')

    def test_cache_flags_do_not_select_requested_variant(self):
        variants, indexed = index_tables(self.tables())
        sibling = resolve(indexed, ancestry(variants, 4))
        self.assertEqual(sibling['Node'][1]['C'], 'sibling')
        self.assertEqual(sibling['Node'][1]['Flag_Variant'], 0)
        self.assertGreater(compare(sibling, active_rows(indexed)), 0)

    def test_invalid_ancestry_is_rejected_including_unselected_branch(self):
        for parent in (99, 3, -1, True):
            with self.subTest(parent=parent):
                tables = self.tables()
                tables['Variant'][1]['ParentVariant_ID'] = parent
                with self.assertRaises(ValueError):
                    index_tables(tables)
        variants, _ = index_tables(self.tables())
        with self.assertRaises(ValueError):
            ancestry(variants, 99)

    def test_duplicate_and_unresolved_row_identities_are_rejected(self):
        for change in ('duplicate_variant', 'duplicate_row', 'unknown_owner', 'nonpositive_id'):
            with self.subTest(change=change):
                tables = self.tables()
                if change == 'duplicate_variant':
                    tables['Variant'].append(variant(1))
                elif change == 'duplicate_row':
                    tables['Node'].append(dict(tables['Node'][0]))
                elif change == 'unknown_owner':
                    tables['Node'][0]['Variant_ID'] = 99
                else:
                    tables['Node'][0]['Node_ID'] = -1
                with self.assertRaises(ValueError):
                    index_tables(tables)

    def test_undocumented_flags_and_ambiguous_active_cache_reject(self):
        tables = self.tables()
        tables['Node'][0]['Flag_Variant'] = 1
        _, indexed = index_tables(tables)
        with self.assertRaises(ValueError):
            active_rows(indexed)
        for flag in (None, -1, 2, True):
            with self.subTest(flag=flag):
                tables['Node'][0]['Flag_Variant'] = flag
                with self.assertRaises(ValueError):
                    index_tables(tables)

    def test_child_rows_and_numeric_union_fail_documented_selection(self):
        variants, indexed = index_tables(self.tables())
        expected = active_rows(indexed)
        self.assertGreater(compare(resolve(indexed, sorted(variants)), expected), 0)
        # Variant2 inherits the root although it has no local rows at all.
        inherited = resolve(indexed, ancestry(variants, 2))
        self.assertEqual(len(inherited['Node']), 1)
        self.assertGreater(compare(resolve(indexed, [2]), inherited), 0)


if __name__ == '__main__':
    unittest.main()
