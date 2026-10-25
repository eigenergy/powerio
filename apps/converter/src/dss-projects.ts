import { available, type InputEntry } from './acquisition';
import { discoveryProblem, inputLimit, type InputProblem } from './input-limits';

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
async function references(files: InputEntry[], cancelled: () => boolean) {
  const byPath = new Map(files.map(file => [file.path, file]));
  const edges = new Map<string, Set<string>>();
  const errors = new Map<string, InputProblem>();
  for (const file of files) if (!available(file)) errors.set(file.path, file.error);
  const budgetErrors: { roots: Set<string>; problem: InputProblem }[] = [];
  // Metadata inventories need no File reads. Process outer entry points first
  // so a nested Master reached by a wrapper is charged before it is read.
  const groups = inventories(files, edges).sort((a, b) => a.root.length - b.root.length);
  const owner = new Map(groups.flatMap(group => group.files.map(file => [file.path, group] as const)));
  const referenced = new Set<string>();
  for (const group of groups) {
    if (cancelled()) break;
    if (group.options.every(file => referenced.has(file.path))) continue;
    const members = new Set(group.files);
    const limit = inputLimit([...members].filter(available));
    if (limit) {
      budgetErrors.push({ roots: new Set(group.options.map(file => file.path)), problem: limit });
      continue;
    }
    const pending = group.files.filter(available).filter(file => /\.dss$/i.test(file.path));
    for (let next = 0; next < pending.length; next++) {
      if (cancelled()) return { edges, errors, budgetErrors };
      const file = pending[next];
      if (edges.has(file.path)) continue;
      const targets = new Set<string>();
      edges.set(file.path, targets);
      let text: string;
      try { text = (await file.file.text()).replace(/\/\*[\s\S]*?\*\//g, ''); }
      catch (error) { errors.set(file.path, discoveryProblem(error)); continue; }
      let base = parent(file.path);
      for (const match of text.matchAll(/^\s*(redirect|compile)\s+(?:file\s*=\s*)?(?:"([^"]+)"|'([^']+)'|\(([^)]+)\)|([^\s!]+))/gim)) {
        const target = relative(base, match[2] ?? match[3] ?? match[4] ?? match[5]);
        if (!target) continue;
        if (match[1].toLowerCase() === 'compile') base = parent(target);
        const child = byPath.get(target);
        if (!child || targets.has(target)) continue;
        targets.add(target); referenced.add(target); members.add(child);
        // Referencing a nested project makes its local companions part of
        // this project. Parent-relative external references add only reached
        // files, preserving the other independent projects' inventories.
        const nested = owner.get(target);
        if (nested && inside(target, group.root)) nested.files.forEach(entry => members.add(entry));
      }
      const problem = inputLimit([...members].filter(available));
      if (problem) { budgetErrors.push({ roots: new Set(group.options.map(file => file.path)), problem }); break; }
      for (const target of targets) {
        const child = byPath.get(target)!;
        if (!edges.has(target) && available(child)) pending.push(child);
      }
    }
  }
  return { edges, errors, budgetErrors };
}

function inventories(files: InputEntry[], edges: Map<string, Set<string>>) {
  const scripts = files.filter(file => /\.dss$/i.test(file.path));
  if (!scripts.length) return [];
  const referenced = new Set([...edges.values()].flatMap(set => [...set]));
  const entries = scripts.filter(file => !referenced.has(file.path));
  const candidates = entries.length ? entries : scripts;
  const named = candidates.filter(file => /^(?:case|master)\.dss$/i.test(name(file.path)));
  const anchors = candidates.filter(file => named.includes(file) ||
    !named.some(other => inside(file.path, parent(other.path))));
  const roots = new Map<string, InputEntry[]>();
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
    return { root, anchor, primary, reachable, options };
  });
  // Loose grid files remain independent when no containing project folder was
  // supplied. Within a project folder, retain every companion regardless of suffix.
  const standalone = /\.(?:m|raw|rawx|json|xiidm|jiidm|xml|aux|epc|uct|pwb|sinx|sqlite|db|zip)$/i;
  return projects.map(project => {
    const inventory = files.filter(file => (!available(file) && file.directory && inside(`${project.anchor}/`, file.path.slice(0, -1))) || project.reachable.has(file.path) ||
      (inside(file.path, project.root) && (project.root !== '' || !standalone.test(file.path)) &&
       !projects.some(other => other !== project && other.root.length > project.root.length && inside(file.path, other.root))));
    let root = project.root;
    for (const path of project.reachable) root = common(root, parent(path));
    return { root, primary: project.primary?.path, files: inventory, options: project.options };
  });
}

export async function dssProjects(files: InputEntry[], cancelled: () => boolean = () => false) {
  if (!files.some(file => /\.dss$/i.test(file.path))) return [];
  const { edges, errors, budgetErrors } = await references(files, cancelled);
  return inventories(files, edges).map(project => ({
    ...project,
    error: inputLimit(project.files.filter(available))
      ?? budgetErrors.find(failure => project.options.some(file => failure.roots.has(file.path)))?.problem
      ?? project.files.map(file => errors.get(file.path)).find(Boolean),
  }));
}
