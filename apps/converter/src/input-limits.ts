import type { BrowserFile } from './types';

export const MAX_INPUT_FILES = 4096;
export const MAX_INPUT_BYTES = 64 * 1024 * 1024;
export interface InputProblem { code: string; message: string }

export function inputLimit(files: Iterable<BrowserFile>): InputProblem | undefined {
  let count = 0, bytes = 0;
  for (const entry of files) {
    count++; bytes += entry.file.size;
    if (count > MAX_INPUT_FILES || bytes > MAX_INPUT_BYTES || !Number.isSafeInteger(entry.file.size) || entry.file.size < 0) {
      return { code: 'WEB.INPUT.LIMIT', message: 'This project exceeds 4096 files or 64 MiB. Choose a smaller project or use PowerIO in the terminal.' };
    }
  }
}

export function discoveryProblem(error: unknown): InputProblem {
  return { code: 'WEB.INPUT.DISCOVERY', message: error instanceof Error ? error.message : 'This project file could not be read. Try again or choose the files again.' };
}
