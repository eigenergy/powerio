import type { BrowserFile } from './types';

const parent = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf('/')));
const name = (path: string) => path.split('/').at(-1)!;
const inside = (path: string, root: string) => !root || path.startsWith(`${root}/`);
const common = (left: string, right: string) => {
  const a = left.split('/').filter(Boolean), b = right.split('/').filter(Boolean);
  let length = 0;
  while (length < a.length && length < b.length && a[length] === b[length]) length++;
  return a.slice(0, length).join('/');
};
function relative(base: string, target: string) {
  if (/^(?:[a-z]:|[/\\])/i.test(target)) return undefined;
  const parts = base.split('/').filter(Boolean);
  for (const part of target.replaceAll('\\', '/').split('/')) {
    if (part === '..') { if (!parts.length) return undefined; parts.pop(); }
    else if (part && part !== '.') parts.push(part);
  }
  return parts.join('/');
}
// Recognize literal include edges for discovery only. The native DSS reader
// still interprets commands, confines references and diagnoses missing files.
async function references(files: BrowserFile[]) {
  const byPath = new Map(files.map(file => [file.path, file]));
  const edges = new Map<string, Set<string>>();
  const pending = files.filter(file => /\.dss$/i.test(file.path));
  while (pending.length) {
    const file = pending.pop()!;
    if (edges.has(file.path)) continue;
    const targets = new Set<string>();
    edges.set(file.path, targets);
    const text = (await file.file.text()).replace(/\/\*[\s\S]*?\*\//g, '');
    let base = parent(file.path);
    for (const match of text.matchAll(/^\s*(redirect|compile)\s+(?:file\s*=\s*)?(?:"([^"]+)"|'([^']+)'|\(([^)]+)\)|([^\s!]+))/gim)) {
      const target = relative(base, match[2] ?? match[3] ?? match[4] ?? match[5]);
      if (!target) continue;
      if (match[1].toLowerCase() === 'compile') base = parent(target);
      const child = byPath.get(target);
      if (child) { targets.add(target); pending.push(child); }
    }
  }
  return edges;
}

export async function dssProjects(files: BrowserFile[]) {
  const scripts = files.filter(file => /\.dss$/i.test(file.path));
  if (!scripts.length) return [];
  const edges = await references(files);
  const referenced = new Set([...edges.values()].flatMap(set => [...set]));
  const entries = scripts.filter(file => !referenced.has(file.path));
  const candidates = entries.length ? entries : scripts;
  const named = candidates.filter(file => /^(?:case|master)\.dss$/i.test(name(file.path)));
  const anchors = candidates.filter(file => named.includes(file) ||
    !named.some(other => inside(file.path, parent(other.path))));
  const roots = new Map<string, BrowserFile[]>();
  for (const file of anchors) roots.set(parent(file.path), [...(roots.get(parent(file.path)) ?? []), file]);
  const projects = [...roots].map(([anchor, options]) => {
    let root = anchor;
    // Strip archive wrappers only while no independent project shares them.
    while (root && parent(root) && ![...roots.keys()].some(other => other !== anchor && inside(`${other}/`, parent(root)))) root = parent(root);
    const caseFile = options.find(file => /^case\.dss$/i.test(name(file.path)));
    const masters = options.filter(file => /^master\.dss$/i.test(name(file.path)));
    const primary = caseFile ?? (masters.length === 1 ? masters[0] : options.length === 1 ? options[0] : undefined);
    const reachable = new Set<string>();
    const pending = options.map(file => file.path);
    while (pending.length) {
      const path = pending.pop()!;
      if (reachable.has(path)) continue;
      reachable.add(path);
      pending.push(...(edges.get(path) ?? []));
    }
    return { root, primary, reachable };
  });
  // Loose grid files remain independent when no containing project folder was
  // supplied. Within a project folder, retain every companion regardless of suffix.
  const standalone = /\.(?:m|raw|rawx|json|xiidm|jiidm|xml|aux|epc|uct|pwb|sinx|sqlite|db|zip)$/i;
  return projects.map(project => {
    const inventory = files.filter(file => project.reachable.has(file.path) ||
      (inside(file.path, project.root) && (project.root !== '' || !standalone.test(file.path)) &&
       !projects.some(other => other !== project && other.root.length > project.root.length && inside(file.path, other.root))));
    let root = project.root;
    for (const path of project.reachable) root = common(root, parent(path));
    return { root, primary: project.primary?.path, files: inventory };
  });
}
