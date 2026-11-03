import type { BrowserFile } from './types';
import type { InputProblem } from './input-limits';

export interface DropEntry {
  isFile: boolean;
  name: string;
  file(success: (file: File) => void, failure: (error: DOMException) => void): void;
  createReader(): { readEntries(success: (entries: DropEntry[]) => void, failure: (error: DOMException) => void): void };
}
export interface UnavailableInput {
  path: string;
  error: InputProblem;
  directory: boolean;
}
export type InputEntry = BrowserFile | UnavailableInput;
export const available = (entry: InputEntry): entry is BrowserFile => 'file' in entry;

export async function acquire(entries: DropEntry[], cancelled: () => boolean = () => false): Promise<InputEntry[]> {
  const result: InputEntry[] = [];
  async function visit(entry: DropEntry, prefix: string) {
    if (cancelled()) return;
    const path = `${prefix}${entry.name}`;
    try {
      if (entry.isFile) {
        const file = await new Promise<File>((resolve, reject) => entry.file(resolve, reject));
        if (!cancelled()) result.push({ path, file });
      } else {
        const reader = entry.createReader();
        while (!cancelled()) {
          const children = await new Promise<DropEntry[]>((resolve, reject) => reader.readEntries(resolve, reject));
          if (!children.length) break;
          for (const child of children) await visit(child, `${path}/`);
        }
      }
    } catch {
      if (!cancelled()) result.push({
        path: entry.isFile ? path : `${path}/`, directory: !entry.isFile,
        error: { code: 'WEB.INPUT.ACQUISITION', message: `Could not acquire ${path}. Remove this case and choose its complete files or folder again. Incomplete projects cannot be converted.` },
      });
    }
  }
  for (const entry of entries) await visit(entry, '');
  return result;
}
