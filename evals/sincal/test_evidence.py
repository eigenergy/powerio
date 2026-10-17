"""Native-package and independently represented SimBench parameter checks.

These tests establish schema evidence. They do not call a PowerIO SINCAL
reader or claim fresh-output acceptance by SINCAL.
"""

import csv
import io
import math
from pathlib import Path
import sqlite3
import unittest
import zipfile

from inspect_native import archive_database, connect, inspect, inspect_database, read_native, rows


FIXTURES = Path(__file__).resolve().parents[2] / 'tests/data/sincal'
NATIVE = FIXTURES / '1-LV-rural1--0-sw.sinx'
SHA256 = '019f52397b4bc5673abed79a5fe1ed0d484d9674ffca04d4102806cd29ccc659'
MARKER = b'[Main]\r\nAppVersion=PSS SINCAL\r\nNetworkType=Electro\r\n'


def archive(entries):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, value in entries:
            z.writestr(name, value)
    return stream.getvalue()


def csv_rows(name):
    with (FIXTURES / 'simbench-csv' / name).open(newline='', encoding='utf-8-sig') as stream:
        return list(csv.DictReader(stream, delimiter=';'))


class NativeInspection(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        _, _, cls.database, _ = read_native(NATIVE)

    def changed(self, sql):
        connection = sqlite3.connect(':memory:')
        try:
            connection.deserialize(self.database)
            connection.executescript(sql)
            return connection.serialize()
        finally:
            connection.close()

    def test_authentic_native_identity_and_contents(self):
        before = NATIVE.read_bytes()
        report = inspect(NATIVE)
        self.assertEqual(report['input_sha256'], SHA256)
        self.assertEqual(report['input_bytes'], 88876)
        self.assertEqual(report['database_sha256'],
                         '43c56ea52c98a15f35909d93fb7fa68f93cc1697662b571c0e5d706033bb435e')
        self.assertEqual(report['version']['Version_No'], 14.8)
        self.assertEqual(report['table_count'], 250)
        self.assertEqual(report['topology'], {'nodes': 15, 'elements': 32, 'terminals': 46})
        for table, count in {'Line': 13, 'Load': 13, 'DCInfeeder': 4,
                             'Infeeder': 1, 'TwoWindingTransformer': 1}.items():
            self.assertEqual(report['nonempty_tables'][table], count)
        self.assertNotIn('ULFNodeResult', report['nonempty_tables'])
        self.assertEqual(NATIVE.read_bytes(), before)
        self.assertFalse(report['electrical_support_verified'])

    def test_inspection_connection_is_read_only(self):
        connection = connect(self.database)
        try:
            with self.assertRaises(sqlite3.OperationalError):
                connection.execute('DELETE FROM Node')
        finally:
            connection.close()

    def test_variant_selection_and_cross_variant_reference(self):
        data = self.changed('INSERT INTO Variant (Variant_ID) VALUES (2);')
        with self.assertRaisesRegex(ValueError, 'select --variant'):
            inspect_database(data)
        self.assertEqual(inspect_database(data, 1)['topology']['nodes'], 15)
        with self.assertRaisesRegex(ValueError, 'unknown variant'):
            inspect_database(data, 3)
        data = self.changed('UPDATE Node SET Variant_ID=2 WHERE Node_ID=1;')
        with self.assertRaisesRegex(ValueError, 'unresolved Terminal'):
            inspect_database(data, 1)

    def test_does_not_guess_variant_inheritance(self):
        data = self.changed('UPDATE Variant SET ParentVariant_ID=2;')
        with self.assertRaisesRegex(ValueError, 'inheritance'):
            inspect_database(data)

    def test_rejects_duplicate_identity_and_terminal_position(self):
        for sql, message in [
            ('INSERT INTO Node SELECT * FROM Node LIMIT 1;',
             'duplicate Node'),
            ('UPDATE Terminal SET TerminalNo=1 WHERE Element_ID=20;', 'terminal position'),
        ]:
            with self.subTest(sql=sql):
                # The fixture itself has unique indexes; drop those only in the
                # private corrupted copy used to test the inspector's checks.
                connection = sqlite3.connect(':memory:')
                connection.deserialize(self.database)
                for name, in connection.execute(
                        "SELECT name FROM sqlite_schema WHERE type='index' AND sql IS NOT NULL").fetchall():
                    connection.execute('DROP INDEX "' + name.replace('"', '""') + '"')
                connection.executescript(sql)
                data = connection.serialize()
                connection.close()
                with self.assertRaisesRegex(ValueError, message):
                    inspect_database(data)

    def test_rejects_non_sincal_and_wal_snapshots(self):
        connection = sqlite3.connect(':memory:')
        connection.execute('CREATE TABLE unrelated (x INTEGER)')
        data = connection.serialize()
        connection.close()
        with self.assertRaisesRegex(ValueError, 'missing SINCAL'):
            inspect_database(data)
        data = bytearray(self.database)
        data[18:20] = b'\x02\x02'
        with self.assertRaisesRegex(ValueError, 'WAL'):
            connect(bytes(data))

    def test_rejects_ambiguous_or_missing_project_database(self):
        for entries in [[], [('one_files/database.db', b'x'), ('two_files/database.db', b'x')],
                        [('one_files/DIA/database.db', b'x')]]:
            with self.subTest(entries=entries):
                with self.assertRaisesRegex(ValueError, 'expected one native'):
                    archive_database(archive([('SIArchive.ini', MARKER), *entries]))

    def test_archive_path_and_decompression_limits(self):
        for name in ['../outside', '/outside', 'C:/outside', 'case/../outside',
                     'case\\outside', 'case//outside']:
            with self.subTest(name=name):
                with self.assertRaisesRegex(ValueError, 'unsafe archive path'):
                    archive_database(archive([(name, b'x')]))
        with self.assertRaisesRegex(ValueError, 'duplicate archive path'):
            archive_database(archive([('Case', b'x'), ('case', b'y')]))
        with self.assertRaisesRegex(ValueError, 'compression ratio'):
            archive_database(archive([('bomb', bytes(1 << 20))]))
        link = zipfile.ZipInfo('linked')
        link.create_system = 3
        link.external_attr = 0o120777 << 16
        with self.assertRaisesRegex(ValueError, 'symbolic link'):
            archive_database(archive([(link, b'outside')]))


class PairedSimBenchParameters(unittest.TestCase):
    """Compare native input against the original CSV, not a self round trip."""

    def setUp(self):
        _, _, database, _ = read_native(NATIVE)
        self.db = connect(database)
        self.addCleanup(self.db.close)
        self.elements = {r['Element_ID']: r for r in rows(self.db, 'Element', 1)}
        self.nodes = {r['Node_ID']: r for r in rows(self.db, 'Node', 1)}
        self.terminals = {(r['Element_ID'], r['TerminalNo']): r
                          for r in rows(self.db, 'Terminal', 1)}
        # The CSV represents closed bus section switches explicitly. Collapse
        # only those closed connections for this equivalence comparison.
        self.parent = {r['id']: r['id'] for r in csv_rows('Node.csv')}
        for switch in csv_rows('Switch.csv'):
            if switch['cond'] == '1':
                self.parent[self.root(switch['nodeB'])] = self.root(switch['nodeA'])

    def root(self, name):
        while name != self.parent[name]:
            name = self.parent[name]
        return name

    def near(self, actual, expected, *, relative=1e-9, absolute=1e-12):
        self.assertTrue(math.isclose(actual, float(expected), rel_tol=relative, abs_tol=absolute),
                        f'{actual} != {expected}')

    def assert_terminal(self, element, number, csv_node):
        native = self.nodes[self.terminals[element, number]['Node_ID']]['Name']
        self.assertEqual(self.root(native), self.root(csv_node))

    def test_loads_and_converter_generators(self):
        for table, file, p, q in [('Load', 'Load.csv', 'pLoad', 'qLoad'),
                                  ('DCInfeeder', 'RES.csv', 'pRES', 'qRES')]:
            expected = {r['id']: r for r in csv_rows(file)}
            actual = rows(self.db, table, 1)
            self.assertEqual(len(actual), len(expected))
            for row in actual:
                eid = row['Element_ID']
                reference = expected[self.elements[eid]['Name']]
                self.near(row['P'] * row['fP'], reference[p])
                self.near(row['Q'] * row['fQ'], reference[q])
                self.assert_terminal(eid, 1, reference['node'])

    def test_bus_voltages_are_inherited_from_voltage_levels(self):
        expected = {r['id']: r for r in csv_rows('Node.csv')}
        levels = {r['VoltLevel_ID']: r for r in rows(self.db, 'VoltageLevel', 1)}
        self.assertEqual(len(self.nodes), 15)
        for node in self.nodes.values():
            # Observed mode only. This does not establish the semantics of
            # other Flag_Volt values or the network-level fallback rules.
            self.assertEqual(node['Flag_Volt'], 0)
            self.assertEqual(node['Un'], 0)
            level = levels[node['VoltLevel_ID']]
            self.near(level['Un'], expected[node['Name']]['vmR'])
            self.near(level['f'], 50)

    def test_line_units_and_connectivity(self):
        expected = {r['id']: r for r in csv_rows('Line.csv')}
        types = {r['id']: r for r in csv_rows('LineType.csv')}
        actual = rows(self.db, 'Line', 1)
        self.assertEqual(len(actual), len(expected))
        for row in actual:
            eid = row['Element_ID']
            reference = expected[self.elements[eid]['Name']]
            kind = types[reference['type']]
            self.near(row['l'], reference['length'])  # km
            self.near(row['r'], kind['r'])  # ohm/km
            self.near(row['x'], kind['x'])  # ohm/km
            self.near(row['Ith'] * 1000, kind['iMax'])  # kA -> A
            self.near(2 * math.pi * row['fn'] * row['c'] * 1e-3, kind['b'])  # nF/km -> uS/km
            self.assert_terminal(eid, 1, reference['nodeA'])
            self.assert_terminal(eid, 2, reference['nodeB'])

    def test_transformer_bases_and_separate_rotation(self):
        expected = {r['id']: r for r in csv_rows('Transformer.csv')}
        types = {r['id']: r for r in csv_rows('TransformerType.csv')}
        actual = rows(self.db, 'TwoWindingTransformer', 1)
        self.assertEqual(len(actual), len(expected))
        for row in actual:
            eid = row['Element_ID']
            reference = expected[self.elements[eid]['Name']]
            kind = types[reference['type']]
            for native, field in [('Un1', 'vmHV'), ('Un2', 'vmLV'), ('Sn', 'sR'),
                                  ('uk', 'vmImp'), ('Vfe', 'pFe'), ('i0', 'iNoLoad'),
                                  ('AddRotate', 'va0')]:
                self.near(row[native], kind[field])
            self.near(row['ur'], float(kind['pCu']) / (float(kind['sR']) * 10))
            self.assert_terminal(eid, 1, reference['nodeHV'])
            self.assert_terminal(eid, 2, reference['nodeLV'])

    def test_stored_balanced_results_agree_with_csv(self):
        expected = {r['node']: r for r in csv_rows('NodePFResult.csv')}
        actual = rows(self.db, 'LFNodeResult', 1)
        self.assertEqual(len(actual), len(expected))
        for row in actual:
            reference = expected[self.nodes[row['Node_ID']]['Name']]
            # CSV results are rounded. These compare two stored results, not
            # a fresh solve; do not promote them to a SINCAL solver oracle.
            self.near(row['U_Un'] / 100, reference['vm'], absolute=1e-5)
            self.near(row['phi'], reference['va'], absolute=1e-3)


if __name__ == '__main__':
    unittest.main()
