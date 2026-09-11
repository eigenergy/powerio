import { BlobReader, BlobWriter, ZipReader, type FileEntry } from '@zip.js/zip.js';
import type { BrowserFile } from './types';

export interface InputCase { name: string; entries: BrowserFile[]; primary?: string; needsPrimary?: boolean }
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
      if (files.length >= 4096 || total > 64 * 1024 * 1024 || member.uncompressedSize > Math.max(1, member.compressedSize) * 200) {
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

export async function groupInputs(entries: BrowserFile[]): Promise<InputCase[]> {
  const result: InputCase[] = [];
  const remaining = new Set(entries);
  const take = (name: string, files: BrowserFile[], root: string, primary?: string, needsPrimary = false) => {
    files.forEach(file => remaining.delete(file));
    const trim = (path: string) => root ? path.slice(root.length + 1) : path;
    result.push({ name, entries: files.map(file => ({ ...file, path: trim(file.path) })), primary: primary ? trim(primary) : undefined, needsPrimary });
  };
  for (const marker of entries.filter(file => basename(file.path).toLowerCase() === 'network.csv')) {
    const root = directory(marker.path);
    take(basename(root) || 'PyPSA project', [...remaining].filter(file => root ? file.path.startsWith(`${root}/`) : !file.path.includes('/')), root);
  }
  const dssRoots = new Set([...remaining].filter(file => /\.dss$/i.test(file.path)).map(file => file.path.includes('/') ? file.path.split('/')[0] : ''));
  for (const root of dssRoots) {
    const files = [...remaining].filter(file => (root ? file.path.startsWith(`${root}/`) : !file.path.includes('/')) && /\.(dss|csv|txt)$/i.test(file.path));
    const dss = files.filter(file => /\.dss$/i.test(file.path));
    const masters = dss.filter(file => /^master\.dss$/i.test(basename(file.path)));
    const primary = masters.length === 1 ? masters[0] : dss.length === 1 ? dss[0] : undefined;
    take(root || primary?.file.name || 'OpenDSS project', files, root, primary?.path, !primary);
  }
  const xmlGroups = new Map<string, BrowserFile[]>();
  for (const file of remaining) {
    if (!/\.xml$/i.test(file.path)) continue;
    const prefix = await file.file.slice(0, 4096).text();
    if (!/<(?:\w+:)?RDF\b/.test(prefix)) continue;
    const root = directory(file.path);
    xmlGroups.set(root, [...(xmlGroups.get(root) ?? []), file]);
  }
  for (const [root, files] of xmlGroups) if (files.length > 1) take(basename(root) || 'CGMES project', files, root);
  for (const file of remaining) result.push({ name: file.file.name, entries: [{ ...file, path: basename(file.path) }], primary: basename(file.path) });
  return result;
}
