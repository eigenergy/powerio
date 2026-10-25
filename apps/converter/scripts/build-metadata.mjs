import { readFile, writeFile } from 'node:fs/promises';
const cargo = await readFile(new URL('../../../Cargo.toml', import.meta.url), 'utf8');
const workspacePackage = cargo.split(/^\[workspace\.package\]\s*$/m)[1]?.split(/^\[/m)[0];
const version = workspacePackage?.match(/^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?)"\s*$/m)?.[1];
if (!version) throw new Error('Missing workspace package version for the converter analytics policy');
const template = await readFile(new URL('./analytics-policy.template.js', import.meta.url), 'utf8');
const marker = '__POWERIO_ENGINE_VERSION__';
if (template.split(marker).length !== 2) throw new Error('Expected one engine-version marker');
await writeFile(new URL('../public/analytics-policy.js', import.meta.url),
  template.replace(marker, JSON.stringify(version)));
