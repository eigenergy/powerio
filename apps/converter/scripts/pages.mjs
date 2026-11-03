import { readFile, writeFile, mkdir, copyFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const html = await readFile(`${root}dist/index.html`, 'utf8');
const pairs = [
  ['psse-to-matpower', 'PSS/E to MATPOWER', 'transmission', 'matpower', 'Convert PSS/E RAW or RAWX cases to MATPOWER. PowerIO reads supported RAW revisions and reports any information the target cannot represent.'],
  ['matpower-to-pandapower', 'MATPOWER to pandapower', 'transmission', 'pandapower-json', 'Convert MATPOWER case files to pandapower JSON. Add one case or a batch, review the conversion diagnostics, and download files locally.'],
  ['matpower-to-powermodels', 'MATPOWER to PowerModels', 'transmission', 'powermodels-json', 'Convert MATPOWER cases to PowerModels JSON for a Julia workflow. Inspect warnings and download a batch without sending files to a server.'],
  ['opendss-to-pmd', 'OpenDSS to PMD', 'distribution', 'pmd-json', 'Convert an OpenDSS feeder to PowerModelsDistribution engineering JSON. Choose a folder or ZIP for projects that reference other files.'],
];
for (const [slug, label, family, token, description] of pairs) {
  const url = `https://powerio.dev/convert/${slug}/`;
  const page = html
    .replace(/<title>.*?<\/title>/, `<title>${label} | PowerIO Convert</title>`)
    .replace(/(<meta name="description" content=")[^"]*/, `$1${description}`)
    .replaceAll('https://powerio.dev/convert/"', `${url}"`)
    .replace('<div id="app"></div>', `<div id="app"></div><section class="landing-description"><h2>${label}</h2><p>${description}</p><p>Files stay on your computer. The same PowerIO parser and writers run in your browser and in the terminal.</p><a href="/convert/">All conversion formats</a></section>`)
    .replace('<html lang="en">', `<html lang="en" data-family="${family}" data-target="${token}">`);
  await mkdir(`${root}dist/${slug}`, { recursive: true });
  await writeFile(`${root}dist/${slug}/index.html`, page);
}
await writeFile(`${root}dist/sitemap.xml`, `<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${['', ...pairs.map(pair => pair[0] + '/')].map(path => `<url><loc>https://powerio.dev/convert/${path}</loc></url>`).join('')}</urlset>`);
await copyFile(new URL('../../../docs/src/assets/powerio-logo.svg', import.meta.url), `${root}dist/powerio-logo.svg`);
