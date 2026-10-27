import { BlobReader, BlobWriter, ZipReader, type FileEntry } from '@zip.js/zip.js';
import type { BrowserFile } from './types';
import { available, type InputEntry } from './acquisition';
import { dssProjects } from './dss-projects';
import { discoveryProblem, inputLimit, MAX_INPUT_BYTES, MAX_INPUT_FILES, type InputProblem } from './input-limits';

export interface InputCase { name: string; entries: BrowserFile[]; paths?: string[]; primary?: string; needsPrimary?: boolean; error?: InputProblem; acquisitionFailed?: boolean }
const directory = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf('/')));
export const basename = (path: string) => path.split('/').at(-1) ?? path;
export function safeName(name: string) {
  const result = name.replace(/[^a-zA-Z0-9_.-]/g, '_').replace(/^\.+/, '').slice(0, 100);
  return /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(result) ? `case-${result}` : result || 'case';
}
export function stem(name: string) { return safeName(basename(name).replace(/\.[^.]+$/, '')); }
function safePath(path: string) {
  if (!path || path.startsWith('/') || path.includes('\\') || path.split('/').some(part => !part || part === '.' || part === '..') || /^[a-z]:/i.test(path)) {
    throw new Error('The archive contains an invalid project path. Extract the project locally and choose its folder.');
  }
  return path;
}

export async function unpack(entry: BrowserFile): Promise<BrowserFile[]> {
  const reader = new ZipReader(new BlobReader(entry.file), { useWebWorkers: false, checkCrc32: true, checkOverlappingEntry: true, strictness: 'strict' });
  try {
    const files: FileEntry[] = [];
    let total = 0;
    for await (const member of reader.getEntriesGenerator()) {
      if (member.directory) continue;
      safePath(member.filename);
      if (member.encrypted) throw new Error('This ZIP is encrypted. Extract it locally and choose the folder.');
      total += member.uncompressedSize;
      if (files.length >= MAX_INPUT_FILES || total > MAX_INPUT_BYTES || member.uncompressedSize > Math.max(1, member.compressedSize) * 200) {
        throw new Error('This archive exceeds the project limit: 4096 files, 64 MiB expanded, or a compression ratio of 200. Choose smaller projects or use PowerIO in the terminal.');
      }
      if (files.some(file => file.filename === member.filename)) throw new Error('The ZIP contains duplicate paths. Choose a folder with unique paths.');
      files.push(member);
    }
    // CGMES acquires its archive as one source and can preserve the exact ZIP bytes.
    if (files.length && files.every(file => /\.xml$/i.test(file.filename))) return [entry];
    const expanded: BrowserFile[] = [];
    for (const member of files) {
      if (/\.zip$/i.test(member.filename)) throw new Error('This ZIP contains another archive. Extract it locally and choose the project folder.');
      const blob = await member.getData!(new BlobWriter());
      if (blob.size !== member.uncompressedSize) throw new Error('An archive entry has an unexpected size.');
      expanded.push({ path: `${stem(entry.path)}/${member.filename}`, file: new File([blob], basename(member.filename)) });
    }
    return expanded;
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
