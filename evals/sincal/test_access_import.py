"""Original synthetic records; no native database or external tool required."""

import json
from pathlib import Path
import sys
import tempfile
import unittest

from import_access import Limits, catalog_name, columns_from_schema, run_bounded, table_rows, write_records


SCHEMA = '''-- Synthetic declaration in the mdb-schema sqlite output dialect.
CREATE TABLE `Load`
 (
    `Element_ID` INTEGER NOT NULL,
    `P` REAL,
    `Name` varchar
);
'''


class AccessRecords(unittest.TestCase):
    def test_excluded_catalog_names_are_not_sql_identifiers(self):
        self.assertEqual(catalog_name('Paste Errors'), 'Paste Errors')
        with self.assertRaises(ValueError):
            catalog_name('Node\nElement')

    def test_preserves_null_zero_numeric_text_and_empty_text(self):
        columns = columns_from_schema(SCHEMA, 'Load')
        rows = table_rows(
            '{"Element_ID":1,"P":0,"Name":"0"}\n'
            '{"Element_ID":2,"Name":""}\n'
            '{"Element_ID":3,"P":1.2345678901234567,"Name":"x\\ny"}\n',
            columns, Limits())
        self.assertEqual(rows, [[1, 0, '0'], [2, None, ''],
                                [3, 1.2345678901234567, 'x\ny']])
        self.assertEqual([column['name'] for column in columns], ['Element_ID', 'P', 'Name'])

    def test_refuses_schema_programs_and_ambiguous_columns(self):
        for schema in [SCHEMA + 'DROP TABLE Load;',
                       SCHEMA.replace('`P` REAL', '`p` REAL,\n `P` REAL'),
                       SCHEMA.replace('`P` REAL', '`P` REAL DEFAULT (random())'),
                       SCHEMA.replace('`Load`', '`Other`')]:
            with self.subTest(schema=schema), self.assertRaises(ValueError):
                columns_from_schema(schema, 'Load')

    def test_refuses_lossy_or_unbounded_rows(self):
        columns = columns_from_schema(SCHEMA, 'Load')
        for row in ['{"P":NaN}', '{"P":Infinity}', '{"P":true}',
                    '{"P":{}}', '{"Missing":0}', '{"P":0,"P":1}',
                    '{"Element_ID":9223372036854775808}']:
            with self.subTest(row=row), self.assertRaises(ValueError):
                table_rows(row, columns, Limits())
        with self.assertRaisesRegex(ValueError, 'row limit'):
            table_rows('{}\n{}', columns, Limits(table_rows=1))
        with self.assertRaisesRegex(ValueError, 'field limit'):
            table_rows('{"Name":"abcdef"}', columns, Limits(field_bytes=3))

    def test_process_errors_output_limits_and_timeout_are_explicit(self):
        def run(code, **kwargs):
            return run_bounded([sys.executable, '-c', code], **kwargs)
        self.assertEqual(run('print("ok")', byte_limit=100, seconds=5), 'ok\n')
        for code, match, seconds in [
            ('raise SystemExit(2)', 'exited 2', 5),
            ('print("x" * 1000)', 'output limit', 5),
            ('import time; time.sleep(10)', 'timed out', 0.05),
            ('import sys; print("warning", file=sys.stderr)', 'diagnostic', 5),
        ]:
            with self.subTest(code=code), self.assertRaisesRegex(ValueError, match):
                run(code, byte_limit=100, seconds=seconds)
        with self.assertRaisesRegex(ValueError, 'unavailable'):
            run_bounded(['/nonexistent/powerio-test-mdb-json'], byte_limit=100, seconds=1)

    def test_output_is_exclusive_and_failures_leave_no_partial_document(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'records.json'
            write_records({'x': 1}, path)
            with self.assertRaises(FileExistsError):
                write_records({'x': 2}, path)
            self.assertEqual(json.loads(path.read_text()), {'x': 1})
            rejected = Path(temporary) / 'too-large.json'
            with self.assertRaisesRegex(ValueError, 'output byte limit'):
                write_records({'x': 'a' * 100}, rejected, Limits(output_bytes=20))
            self.assertFalse(rejected.exists())
            self.assertEqual(list(Path(temporary).iterdir()), [path])


if __name__ == '__main__':
    unittest.main()
