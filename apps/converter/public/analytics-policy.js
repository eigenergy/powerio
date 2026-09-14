// Both the converter and its isolated analytics frame apply this closed vocabulary.
// Electrical diagnostic identities, component properties, and free text are excluded.
(() => {
  const formats = ['unknown', 'matpower', 'psse', 'psse34', 'psse35', 'psse-rawx',
    'powermodels-json', 'pandapower-json', 'pypsa-csv', 'egret-json', 'powerworld',
    'pslf', 'xiidm', 'jiidm', 'cgmes', 'ucte', 'surge-json', 'ieee-cdf', 'pwb',
    'opfdata-json', 'goc3-json', 'dss', 'pmd-json', 'bmopf-json',
    'bmopf-json@0.1.0', 'bmopf-json@0.2.0'];
  const codes = [
    'PARSE.MATPOWER.MALFORMED', 'PARSE.SOURCE.MALFORMED', 'PARSE.GOC3.MALFORMED',
    'PARSE.XIIDM.VERSION_UNSUPPORTED', 'PARSE.IEEE_CDF.MALFORMED',
    'PARSE.DSS.SOURCE_MALFORMED', 'PARSE.DIST.MALFORMED', 'PARSE.DIST.SOURCE_MALFORMED',
    'READ.PSLF.SOURCE_MALFORMED', 'READ.IEEE_CDF.SOURCE_MALFORMED',
    'READ.PMD.SOURCE_MALFORMED', 'READ.BMOPF.SOURCE_MALFORMED',
    'READ.BMOPF.SCHEMA_ABSENT', 'READ.BMOPF.SCHEMA_UNKNOWN', 'READ.BMOPF.SCHEMA_MISMATCH',
    'READ.XIIDM.VERSION_COMPATIBILITY', 'READ.GOC3.AMBIGUOUS_DOCUMENTS',
    'READ.GOC3.PROBLEM_REQUIRED', 'READ.GOC3.SOURCE_UNRECOGNIZED', 'READ.GOC3.INVALID_DOCUMENT',
    'READ.DSS.INCLUDE_LOAD_FAILED', 'READ.DSS.INCLUDE_DEPTH_LIMIT',
    'READ.DSS.INCLUDE_REFUSED', 'READ.DSS.INCLUDE_BUDGET',
    'READ.IO.READ', 'READ.IO.ALLOCATION_REFUSED', 'READ.IO.REFERENCE_BUDGET', 'READ.DIST.IO_FAILED',
    'REQUEST.SOURCE.INVALID_NAME', 'REQUEST.SOURCE.INVALID_PATH',
    'REQUEST.SOURCE.DIRECTORY_REQUIRED', 'REQUEST.SOURCE.ESCAPES_ROOT', 'REQUEST.SOURCE.UNKNOWN_BUFFER',
    'REQUEST.FORMAT.INVALID_ID', 'REQUEST.FORMAT.UNKNOWN', 'REQUEST.FORMAT.WRITE_UNSUPPORTED',
    'REQUEST.DIST_FORMAT.UNKNOWN', 'REQUEST.PARSE.POWERIO_IR',
    'REQUEST.OUTPUT.INVALID_ARTIFACT_PATH', 'REQUEST.OUTPUT.INVALID_LAYOUT', 'REQUEST.OUTPUT.DUPLICATE_ARTIFACT',
    'EMIT.FORMAT.REQUIRED_VALUE_MISSING',
    'WEB.INPUT.ARCHIVE', 'WEB.INPUT.VALUE_TYPE', 'WEB.INPUT.DUPLICATE_PATH', 'WEB.INPUT.LIMIT',
    'WEB.CONVERT.FAILED', 'WEB.CONVERT.WORKER', 'WEB.CONVERT.NOT_PARSED', 'WEB.STORAGE.FULL',
  ];
  const counts = ['1', '2-10', '11-100', '101-plus'];
  const outcomes = ['ready', 'converted', 'unchanged', 'warnings', 'error', 'needs-files', 'needs-primary'];
  const operation = { source: formats, target: formats, outcome: outcomes };
  const schemas = {
    example: { kind: ['transmission', 'distribution', 'mixed'] },
    batch_started: { count: counts, version: ['0.11.1'] },
    batch_completed: { count: counts, duration: ['under-10s', '10s-plus'] },
    batch_cancelled: {},
    parse_result: operation,
    convert_result: operation,
    input_result: { outcome: ['error', 'needs-primary'] },
    engine_result: { outcome: ['ready', 'error'], version: ['0.11.1'] },
    problem: { source: formats, target: formats, stage: ['input', 'parse', 'convert'], code: [...codes, 'other'] },
    download: { kind: ['single', 'batch'], target: formats, count: counts },
    settings_shared: {}, cli_opened: {}, issue_opened: {},
  };
  const isRecord = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  const safeFormat = value => formats.includes(value) ? value : 'unknown';
  const safeCode = value => codes.includes(value) ? value : 'other';
  const normalize = (name, input = {}) => {
    if (typeof name !== 'string' || !Object.hasOwn(schemas, name) || !isRecord(input)) return null;
    const data = {};
    for (const [key, values] of Object.entries(schemas[name])) {
      if (!Object.hasOwn(input, key)) continue;
      if (values.includes(input[key])) data[key] = input[key];
      else if (key === 'source' || key === 'target') data[key] = 'unknown';
      else if (key === 'code') data[key] = 'other';
    }
    return { name, data };
  };
  const createBudget = () => {
    let total = 0;
    const problems = new Set();
    return event => {
      if (total >= 100) return false;
      if (event.name === 'problem') {
        const key = JSON.stringify(event.data);
        if (problems.has(key) || problems.size >= 20) return false;
        problems.add(key);
      }
      total++;
      return true;
    };
  };
  globalThis.powerioAnalyticsPolicy = Object.freeze({ normalize, safeFormat, safeCode, createBudget });
})();
