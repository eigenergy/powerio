import { BlobReader, BlobWriter, ZipReader, type Entry, type FileEntry } from '@zip.js/zip.js';
import type { BrowserFile, CollectionCandidate, CollectionSelection } from './types';
import { available, type InputEntry } from './acquisition';
import { dssProjects } from './dss-projects';
import { discoveryProblem, inputLimit, MAX_INPUT_BYTES, MAX_INPUT_FILES, type InputProblem } from './input-limits';

export interface InputCase { selection?: CollectionSelection; collectionKey?: string; name: string; entries: BrowserFile[]; paths?: string[]; primary?: string; needsPrimary?: boolean; error?: InputProblem; acquisitionFailed?: boolean }
const directory = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf('/')));
export const basename = (path: string) => path.split('/').at(-1) ?? path;
export function safeName(name: string) {
  const result = name.replace(/[^a-zA-Z0-9_.-]/g, '_').replace(/^\.+/, '').slice(0, 100);
  return /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(result) ? `case-${result}` : result || 'case';
}
export function stem(name: string) { return safeName(basename(name).replace(/\.[^.]+$/, '')); }
function safePath(path: string) {
  const parts = path.split('/');
  if (!path || new TextEncoder().encode(path).length > 4096 || parts.length > 64 ||
      /[\\:\x00-\x1f\x7f<>"|?*]/.test(path) || parts.some(part => !part || part === '.' || part === '..' ||
        /[. ]$/.test(part) || /^(con|prn|aux|nul|com[0-9¹²³]|lpt[0-9¹²³])(?:\.|$)/i.test(part))) {
    throw new Error('The archive contains an invalid or nonportable project path.');
  }
  return path;
}

function hasRdfRoot(text: string): boolean {
  // The payload is already bounded. Skip leading declarations/comments so a
  // long comment does not hide RDF, or make an XIIDM network look like RDF.
  for (;;) {
    text = text.trimStart();
    const end = text.startsWith('<!--') ? '-->' : text.startsWith('<?') ? '?>' : undefined;
    if (!end) return /^<(?:[\w.-]+:)?RDF(?=[\s/>])/.test(text);
    const at = text.indexOf(end);
    if (at < 0) return false;
    text = text.slice(at + end.length);
  }
}

export async function unpack(entry: BrowserFile, cancelled: () => boolean = () => false, root = stem(entry.path)): Promise<BrowserFile[]> {
  const limit = inputLimit([entry]);
  if (limit) throw new Error(limit.message);
  const reader = new ZipReader(new BlobReader(entry.file), { useWebWorkers: false, checkCrc32: true, checkOverlappingEntry: true, strictness: 'strict' });
  try {
    const files: FileEntry[] = [];
    const directories: Entry[] = [];
    let total = 0, count = 0;
    const names = new Map<string, { id: number; spelling: string; directory: boolean; explicit: boolean }>();
    for await (const member of reader.getEntriesGenerator()) {
      if (cancelled()) throw new Error('Archive acquisition cancelled.');
      if (++count > MAX_INPUT_FILES) throw new Error('This archive exceeds 4096 entries.');
      if (new TextDecoder('utf-8', { fatal: true }).decode(member.rawFilename) !== member.filename) throw new Error('ZIP filenames must retain their exact UTF-8 spelling.');
      const path = safePath(member.directory ? member.filename.replace(/\/$/, '') : member.filename);
      const kind = (member.unixMode ?? 0) & 0o170000;
      if (![0, 0o100000, 0o040000].includes(kind) || (kind !== 0 && (kind === 0o040000) !== member.directory)) throw new Error('ZIP links and special files are unsupported.');
      if (member.directory && (member.uncompressedSize !== 0 || member.signature !== 0)) throw new Error('Invalid ZIP directory payload.');
      const parts = path.split('/');
      let parent = 0;
      for (let index = 0; index < parts.length; index++) {
        const spelling = parts[index];
        const key = `${parent}:${spelling.toLowerCase()}`;
        const directory = member.directory || index + 1 < parts.length;
        const explicit = index + 1 === parts.length;
        const previous = names.get(key);
        if (previous && (previous.spelling !== spelling || previous.directory !== directory || (explicit && previous.explicit))) throw new Error('The ZIP contains duplicate or conflicting paths.');
        const id = previous?.id ?? names.size + 1;
        names.set(key, { id, spelling, directory, explicit: explicit || !!previous?.explicit });
        parent = id;
      }
      if (member.encrypted) throw new Error('This ZIP is encrypted. Extract it locally and choose the folder.');
      if (![0, 8].includes(member.compressionMethod) || !Number.isSafeInteger(member.uncompressedSize) || !Number.isSafeInteger(member.compressedSize) || member.uncompressedSize < 0 || member.compressedSize < 0) throw new Error('Unsupported ZIP member metadata.');
      if (member.directory) { directories.push(member); continue; }
      total += member.uncompressedSize;
      if (files.length >= MAX_INPUT_FILES || total > MAX_INPUT_BYTES || member.uncompressedSize > Math.max(1, member.compressedSize) * 200) {
        throw new Error('This archive exceeds the project limit: 4096 files, 64 MiB expanded, or a compression ratio of 200. Choose smaller projects or use PowerIO in the terminal.');
      }
      files.push(member);
    }
    const checkCancelled = () => { if (cancelled()) throw new Error('Archive acquisition cancelled.'); };
    // zip.js 2.14 attaches getData to directories as well, although its type
    // declarations expose it only on FileEntry. Verify their headers and EOF.
    for (const member of directories) {
      checkCancelled();
      const blob = await (member as Entry & Pick<FileEntry, 'getData'>).getData(new BlobWriter(), { onprogress: checkCancelled });
      if (blob.size !== 0) throw new Error('Invalid ZIP directory payload.');
    }
    let nativeCgmes = files.length > 0 && files.every(file => /\.xml$/i.test(file.filename));
    const expanded: BrowserFile[] = [];
    for (const member of files) {
      checkCancelled();
      if (/\.zip$/i.test(member.filename)) throw new Error('This ZIP contains another archive. Extract it locally and choose the project folder.');
      const blob = await member.getData(new BlobWriter(), { onprogress: checkCancelled });
      if (blob.size !== member.uncompressedSize) throw new Error('An archive entry has an unexpected size.');
      if (nativeCgmes && !hasRdfRoot(await blob.text())) nativeCgmes = false;
      expanded.push({ collectionRoot: root, path: `${root}/${member.filename}`, file: new File([blob], basename(member.filename)) });
    }
    checkCancelled();
    // Keep native RDF profile archives byte exact; XML suffixes alone do not
    // imply CGMES (a transport ZIP can hold several independent XIIDM cases).
    return nativeCgmes ? [{ ...entry, collectionRoot: root, path: `${root}/${basename(entry.path)}` }] : expanded;
  } finally { await reader.close(); }
}

export async function groupInputs(entries: InputEntry[], cancelled: () => boolean = () => false): Promise<InputCase[]> {
  const result: InputCase[] = [];
  const remaining = new Set(entries);
  const unavailable = entries.filter(entry => !available(entry));
  const take = (name: string, files: InputEntry[], root: string, primary?: string, needsPrimary = false, error?: InputProblem) => {
    files.forEach(file => remaining.delete(file));
    const trim = (path: string) => root && path.startsWith(`${root}/`) ? path.slice(root.length + 1) : path;
    const acquired = files.filter(available);
    const missing = files.find(file => !available(file));
    result.push({ name, entries: acquired.map(file => ({ ...file, path: trim(file.path) })), paths: files.map(file => trim(file.path) || './'), primary: primary ? trim(primary) : undefined, needsPrimary, error: missing && !available(missing) ? missing.error : error ?? inputLimit(acquired), acquisitionFailed: !!missing });
  };
  for (const marker of entries.filter(file => basename(file.path).toLowerCase() === 'network.csv')) {
    const root = directory(marker.path);
    const files = [...remaining].filter(file => root ? file.path.startsWith(`${root}/`) : !file.path.includes('/'));
    // A failed ancestor listing can affect several projects; consuming the
    // error for one case must not make the others appear complete.
    take(basename(root) || 'PyPSA project', [...new Set([...files, ...unavailable.filter(file => !available(file) && file.directory && `${root}/`.startsWith(file.path))])], root);
  }
  for (const project of await dssProjects([...new Set([...remaining, ...unavailable.filter(file => !available(file) && file.directory)])], cancelled)) {
    take(basename(project.root) || project.primary || 'OpenDSS project', project.files,
      project.root, project.primary, !project.primary, project.error);
  }
  const xmlGroups = new Map<string, BrowserFile[]>();
  for (const file of remaining) {
    if (cancelled()) return result;
    if (!available(file)) continue;
    if (!/\.xml$/i.test(file.path)) continue;
    const limit = inputLimit([file]);
    if (limit) { take(file.file.name, [file], directory(file.path), file.path, false, limit); continue; }
    let prefix: string;
    try { prefix = await file.file.slice(0, 4096).text(); }
    catch (error) { take(file.file.name, [file], directory(file.path), file.path, false, discoveryProblem(error)); continue; }
    if (!/<(?:\w+:)?RDF\b/.test(prefix)) continue;
    const root = directory(file.path);
    xmlGroups.set(root, [...(xmlGroups.get(root) ?? []), file]);
  }
  for (const [root, files] of xmlGroups) {
    const missing = unavailable.filter(file => !available(file) && (directory(file.path) === root || (file.directory && `${root}/`.startsWith(file.path))));
    if (files.length > 1 || missing.length) take(basename(root) || 'CGMES project', [...files, ...missing], root);
  }
  for (const file of remaining) take(available(file) ? file.file.name : basename(file.path.replace(/\/$/, '')), [file], directory(file.path), file.path);
  return result;
}


async function legacyGroups(entries: InputEntry[], cancelled: () => boolean): Promise<InputCase[]> {
  const roots = new Set(entries.filter(available).map(e => e.collectionRoot));
  const result: InputCase[] = [];
  for (const root of roots) {
    const selected = entries.filter(e => available(e) ? e.collectionRoot === root : !root);
    const scoped = root ? selected.map(e => ({ ...e, path: e.path.slice(root.length + 1) })) : selected;
    result.push(...await groupInputs(scoped, cancelled));
  }
  if (!roots.size) return groupInputs(entries, cancelled);
  return result;
}

/** Shared facade discovery for fully acquired collections. Legacy XML grouping
 * remains separate until its explicit document-group chooser is exposed. */
export async function groupCollectionInputs(entries: InputEntry[], discover: (files: BrowserFile[]) => Promise<{ ok?: boolean; candidates?: CollectionCandidate[]; diagnostics?: { message: string }[] }>, collectionKey: string, cancelled: () => boolean): Promise<InputCase[]> {
  if (entries.some(entry => !available(entry)) || inputLimit(entries.filter(available))) return legacyGroups(entries, cancelled);
  const files = entries.filter(available);
  let result;
  try { result = await discover(files); }
  catch { return cancelled() ? [] : legacyGroups(entries, cancelled); }
  if (cancelled()) return [];
  // Older worker bundles still use the established path during a cached-page upgrade.
  if (!result.candidates && result.ok !== false) return legacyGroups(entries, cancelled);
  if (!result.ok) return [{ name: 'Input collection', entries: files, error: { code: 'WEB.INPUT.COLLECTION', message: result.diagnostics?.map(d => d.message).join('\n') || 'Collection discovery failed.' } }];
  const shared = result.candidates!.filter(c => c.status !== 'needs-options');
  if (!shared.length) return legacyGroups(entries, cancelled);
  // XML/native options keep the existing chooser. Shared candidates always retain
  // access to the whole supplied tree, including files independently selectable.
  const claimed = new Set(shared.map(c => c.selection.primary).filter(Boolean));
  const paths = files.map(f => f.path);
  const sortedPaths = [...paths].sort();
  const owners = new Map(files.map(f => [f.path, f.collectionRoot]));
  const inventories = new Map<string, string[]>([['', paths]]);
  const scopePaths = (root: string | null) => {
    const key = root || '';
    const cached = inventories.get(key);
    if (cached) return cached;
    const prefix = `${key}/`;
    let lo = 0, hi = sortedPaths.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (sortedPaths[mid] < prefix) lo = mid + 1; else hi = mid;
    }
    const scoped: string[] = [];
    for (let i = lo; i < sortedPaths.length && sortedPaths[i].startsWith(prefix); i++) scoped.push(sortedPaths[i].slice(prefix.length));
    inventories.set(key, scoped);
    return scoped;
  };
  const cases: InputCase[] = shared.map(c => {
    let selection = c.selection;
    const owner = owners.get(selection.primary || '');
    // A transport ZIP is one namespace, even when uploaded beside other ZIPs.
    if (owner && !selection.root) selection = { ...selection, root: owner, primary: selection.primary!.slice(owner.length + 1) };
    return { name: basename(c.selection.primary || c.selection.root || 'dataset'), entries: files, paths: scopePaths(selection.root), primary: selection.primary || undefined, selection, collectionKey };
  });
  const fallback = files.filter(f => !claimed.has(f.path) && /\.(xml|zip|sinx)$/i.test(f.path));
  return [...cases, ...await legacyGroups(fallback, cancelled)];
}
