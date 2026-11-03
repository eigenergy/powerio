import { BlobReader, BlobWriter, ZipWriter } from '@zip.js/zip.js';

export class ArtifactStore {
  private root?: FileSystemDirectoryHandle;
  private parent?: FileSystemDirectoryHandle;
  private session = crypto.randomUUID();
  private memory = new Map<string, Blob>();
  private memorySize = 0;
  private releaseLock?: () => void;

  async initialize() {
    try {
      if (!navigator.storage?.getDirectory || !navigator.locks) return;
      const parent = await (await navigator.storage.getDirectory()).getDirectoryHandle('powerio-convert', { create: true });
      await navigator.locks.request('powerio-convert-cleanup', async () => {
        const held = await navigator.locks.query();
        const active = new Set(held.held?.map(lock => lock.name));
        for await (const name of (parent as any).keys()) {
          if (!active.has(`powerio-convert-${name}`)) await parent.removeEntry(name, { recursive: true });
        }
        await new Promise<void>(resolve => {
          void navigator.locks.request(`powerio-convert-${this.session}`, async () => {
            resolve();
            await new Promise<void>(release => { this.releaseLock = release; });
          });
        });
        this.parent = parent;
        this.root = await parent.getDirectoryHandle(this.session, { create: true });
      });
    } catch { this.root = undefined; }
  }

  async put(key: string, bytes: Uint8Array | Blob) {
    const blob = bytes instanceof Blob ? bytes : new Blob([bytes as Uint8Array<ArrayBuffer>]);
    if (this.root) {
      const file = await this.root.getFileHandle(key, { create: true });
      const writer = await file.createWritable();
      try { await writer.write(blob); await writer.close(); }
      catch (error) { await writer.abort(); throw error; }
    } else {
      const total = this.memorySize - (this.memory.get(key)?.size ?? 0) + blob.size;
      if (total > 128 * 1024 * 1024) throw new Error('Local storage is full. Download completed results, clear them, and continue with the remaining cases.');
      this.memorySize = total;
      this.memory.set(key, blob);
    }
  }

  async get(key: string): Promise<Blob> {
    if (this.root) return (await this.root.getFileHandle(key)).getFile();
    const blob = this.memory.get(key);
    if (!blob) throw new Error('This result is no longer available. Convert the case again.');
    return blob;
  }

  async remove(key: string) {
    if (this.root) await this.root.removeEntry(key).catch(() => undefined);
    const blob = this.memory.get(key);
    this.memorySize -= blob?.size ?? 0;
    this.memory.delete(key);
  }

  async zip(entries: { key: string; path: string }[], report?: string, cancelled = () => false): Promise<Blob> {
    const file = this.root ? await this.root.getFileHandle('download.zip', { create: true }) : undefined;
    const writer = file ? await file.createWritable() : undefined;
    const zip = new ZipWriter(writer ?? new BlobWriter('application/zip'), { useWebWorkers: false });
    try {
      for (const entry of entries) {
        if (cancelled()) throw new Error('Cancelled');
        await zip.add(entry.path, new BlobReader(await this.get(entry.key)));
      }
      if (cancelled()) throw new Error('Cancelled');
      if (report) await zip.add('conversion-report.json', new BlobReader(new Blob([report], { type: 'application/json' })));
      const blob = await zip.close();
      return file ? await file.getFile() : blob as Blob;
    } catch (error) {
      await writer?.abort().catch(() => undefined);
      throw error;
    }
  }

  async clear() {
    this.memory.clear();
    this.memorySize = 0;
    if (this.parent && this.root) {
      await this.parent.removeEntry(this.session, { recursive: true });
      this.root = await this.parent.getDirectoryHandle(this.session, { create: true });
    }
  }

  dispose() { this.releaseLock?.(); }
}

export function download(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
