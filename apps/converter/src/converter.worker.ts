import init, { capabilities, Conversion } from './wasm/powerio_wasm';
import type { BrowserFile } from './types';
import { inputLimit } from './input-limits';

let conversion: Conversion | undefined;
self.onmessage = async ({ data }) => {
  const { id, type } = data;
  try {
    let result: any;
    const transfers: ArrayBuffer[] = [];
    if (type === 'initialize') {
      await init({ module_or_path: data.module });
      result = JSON.parse(capabilities());
    } else if (type === 'inspect') {
      conversion?.free();
      conversion = new Conversion(data.name);
      const files = data.files as BrowserFile[];
      const limit = inputLimit(files);
      if (limit) throw new Error(limit.message);
      for (const entry of files) {
        const added = JSON.parse(conversion.add_file(entry.path, new Uint8Array(await entry.file.arrayBuffer())));
        if (!added.ok) { result = added; break; }
      }
      result ??= JSON.parse(conversion.inspect(data.primary, data.format));
    } else if (type === 'emit') {
      if (!conversion) throw new Error('Choose a case before converting.');
      result = JSON.parse(conversion.emit(data.format, data.name));
      result.artifacts = [];
      if (result.ok) {
        for (let index = 0; index < conversion.artifact_count(); index++) {
          const path = conversion.artifact_name(index);
          const bytes = conversion.take_artifact(index);
          result.artifacts.push({ path, bytes });
          transfers.push(bytes.buffer as ArrayBuffer);
        }
      }
    } else if (type === 'release') {
      conversion?.free();
      conversion = undefined;
      result = {};
    } else throw new Error('Unknown conversion operation.');
    self.postMessage({ id, result }, { transfer: transfers });
  } catch (error) {
    self.postMessage({ id, error: error instanceof Error ? error.message : 'PowerIO could not finish this operation.' });
  }
};
