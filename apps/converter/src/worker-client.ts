import ConverterWorker from './converter.worker?worker&inline';
import wasmUrl from './wasm/powerio_wasm_bg.wasm?url';

let compiled: Promise<WebAssembly.Module> | undefined;
function moduleBytes() {
  return compiled ??= fetch(wasmUrl).then(async response => {
    if (!response.ok) throw new Error('PowerIO could not load. Reload the page to try again.');
    return WebAssembly.compile(await response.arrayBuffer());
  }).catch(error => { compiled = undefined; throw error; });
}

export class WorkerClient {
  private worker = new ConverterWorker();
  private sequence = 0;
  private pending = new Map<number, { resolve: (value: any) => void; reject: (error: Error) => void }>();
  constructor() {
    this.worker.onmessage = ({ data }) => {
      const pending = this.pending.get(data.id);
      if (!pending) return;
      this.pending.delete(data.id);
      if (data.error) pending.reject(new Error(data.error));
      else pending.resolve(data.result);
    };
    this.worker.onerror = event => {
      event.preventDefault();
      this.close('PowerIO stopped while processing this case. Try a smaller project or use the terminal.');
    };
  }
  async initialize() { return this.request('initialize', { module: await moduleBytes() }); }
  request(type: string, payload: Record<string, unknown> = {}): Promise<any> {
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, type, ...payload });
    });
  }
  close(message = 'Cancelled') {
    this.worker.terminate();
    for (const pending of this.pending.values()) pending.reject(new Error(message));
    this.pending.clear();
  }
}
