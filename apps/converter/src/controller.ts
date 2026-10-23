import { ArtifactStore, download } from './artifacts';
import { analyticsEnabled, bucket, configureAnalytics, track, trackOperation } from './analytics';
import { basename, groupInputs, safeName, stem, unpack } from './inputs';
import { WorkerClient } from './worker-client';
import type { BrowserFile, ConverterState, Diagnostic, Family, Job, Output } from './types';

interface SavedArtifact { key: string; path: string; size: number }
const issue = (message: string, code = 'WEB.CONVERT.FAILED'): Diagnostic => ({ code, severity: 'error', message });
const message = (error: unknown) => error instanceof Error ? error.message : 'This operation could not finish.';
const browserVersion = () => navigator.userAgent.match(/(?:Firefox|Edg|Chrome|Version)\/\d+/)?.[0] ?? 'unknown';
const quote = (value: string) => `'${value.replace(/'/g, "'\\''")}'`;
const warned = (entries: Diagnostic[]) => entries.some(entry => entry.severity === 'warning' || entry.severity === 'error');

export class ConverterController {
  state: ConverterState = {
    formats: [], engine: { version: '', commit: __BUILD_COMMIT__ }, jobs: [],
    targets: { transmission: ['matpower'], distribution: ['pmd-json'] },
    phase: 'loading', activeName: '', message: '', error: '', analytics: analyticsEnabled(),
  };
  private listeners = new Set<(state: ConverterState) => void>();
  private inputs = new Map<string, BrowserFile[]>();
  private artifacts = new Map<string, SavedArtifact[]>();
  private store = new ArtifactStore();
  private client?: WorkerClient;
  private epoch = 0;
  private startup?: Promise<void>;
  private chain: Promise<void> = Promise.resolve();

  subscribe(listener: (state: ConverterState) => void) {
    this.listeners.add(listener); listener(this.state);
    return () => { this.listeners.delete(listener); };
  }
  private publish() {
    this.state = { ...this.state, jobs: this.state.jobs.map(job => ({ ...job, outputs: [...job.outputs] })) };
    this.listeners.forEach(listener => listener(this.state));
  }
  initialize() {
    return this.startup ??= (async () => {
      configureAnalytics(this.state.analytics);
      try {
        const [capabilities] = await Promise.all([this.engine(), this.store.initialize()]);
        this.state.formats = capabilities.formats;
        this.state.engine.version = capabilities.version;
        this.applySharedSettings();
        track('engine_result', { outcome: 'ready', version: this.state.engine.version });
      } catch (error) { this.state.error = message(error); track('engine_result', { outcome: 'error' }); }
      this.state.phase = 'idle'; this.publish();
    })();
  }
  private async engine() {
    if (!this.client) {
      this.client = new WorkerClient();
      try {
        const capabilities = await this.client.initialize();
        this.state.formats = capabilities.formats;
        this.state.engine.version = capabilities.version;
        return capabilities;
      }
      catch (error) { this.client.close(); this.client = undefined; throw error; }
    }
    return { version: this.state.engine.version, formats: this.state.formats };
  }
  private run(task: (epoch: number) => Promise<void>) {
    const epoch = this.epoch;
    const result = this.chain.then(async () => {
      await this.initialize();
      if (epoch !== this.epoch) return;
      this.state.error = ''; this.state.message = '';
      try { await this.engine(); await task(epoch); }
      catch (error) { if (epoch === this.epoch) this.state.error = message(error); }
      finally { if (epoch === this.epoch) { this.state.phase = 'idle'; this.state.activeName = ''; this.publish(); } }
    });
    this.chain = result.catch(() => undefined);
    return result;
  }
  addFiles(files: File[]) {
    return this.addEntries(files.map(file => ({ file, path: file.webkitRelativePath || file.name })));
  }
  addEntries(entries: BrowserFile[]) {
    return this.run(async epoch => {
      this.state.phase = 'inspecting'; this.publish();
      const expanded: BrowserFile[] = [];
      for (const entry of entries) {
        if (epoch !== this.epoch) return;
        try { expanded.push(...(/\.zip$/i.test(entry.path) ? await unpack(entry) : [entry])); }
        catch (error) {
          this.state.jobs.push({ id: crypto.randomUUID(), name: entry.file.name, files: [entry.path], size: entry.file.size, status: 'error', diagnostics: [issue(message(error), 'WEB.INPUT.ARCHIVE')], outputs: [] });
          trackOperation('input', 'error', undefined, undefined, [issue('', 'WEB.INPUT.ARCHIVE')]);
        }
      }
      for (const input of await groupInputs(expanded)) {
        if (epoch !== this.epoch) return;
        const job: Job = {
          id: crypto.randomUUID(), name: input.name, files: input.entries.map(entry => entry.path),
          size: input.entries.reduce((sum, entry) => sum + entry.file.size, 0), primary: input.primary,
          status: input.needsPrimary ? 'needs-primary' : 'queued', diagnostics: [], outputs: [],
        };
        this.inputs.set(job.id, input.entries);
        this.state.jobs.push(job);
        this.publish();
        if (!input.needsPrimary) await this.inspect(job, epoch);
        else trackOperation('input', 'needs-primary');
      }
    });
  }
  private job(id: string) { return this.state.jobs.find(job => job.id === id); }
  private async inspect(input: Job, epoch: number, release = true) {
    const id = input.id;
    const job = this.job(id)!;
    job.status = 'inspecting'; this.state.activeName = job.name; this.publish();
    try {
      const result = await this.client!.request('inspect', { name: job.name, files: this.inputs.get(id), primary: job.primary, format: job.overrideFormat || undefined });
      if (epoch !== this.epoch) return false;
      const current = this.job(id)!;
      current.diagnostics = result.diagnostics ?? [];
      current.format = result.format ?? job.overrideFormat;
      current.family = result.family ?? this.state.formats.find(format => format.token === current.format)?.family;
      current.valueType = result.valueType;
      current.status = result.ok ? 'ready' : 'error';
      if (current.diagnostics.some(entry => entry.severity === 'error')) current.status = 'error';
      else if (current.diagnostics.some(entry => ['READ.DSS.INCLUDE_LOAD_FAILED', 'READ.DSS.INCLUDE_DEPTH_LIMIT'].includes(entry.code))) current.status = 'needs-files';
      if (result.ok && !current.family) {
        current.status = 'error';
        current.diagnostics.push(issue('This document is not a supported grid exchange case. Use PowerIO in the terminal for geographic layers, IR, and other operations.', 'WEB.INPUT.VALUE_TYPE'));
      }
      if (release) {
        trackOperation('parse', current.status === 'ready' && warned(current.diagnostics) ? 'warnings' : current.status, current.format, undefined, current.diagnostics);
        await this.client!.request('release');
      }
      this.publish();
      return current.status === 'ready';
    } catch (error) {
      if (epoch !== this.epoch) return false;
      const current = this.job(id)!;
      current.status = 'error'; current.diagnostics = [issue(message(error))];
      trackOperation('parse', 'error', current.format, undefined, current.diagnostics);
      this.client?.close(); this.client = undefined;
      await this.engine(); this.publish();
      return false;
    }
  }
  setTargets(family: Family, tokens: string[]) {
    this.state.targets[family] = tokens.filter(token => this.state.formats.some(format => format.token === token && format.family === family && format.canEmit));
    this.publish();
  }
  setJobTargets(id: string, tokens: string[] | undefined) {
    const job = this.job(id); if (!job) return;
    job.targets = tokens?.filter(token => this.state.formats.some(format => format.token === token && format.family === job.family && format.canEmit));
    this.publish();
  }
  setFormat(id: string, token: string) {
    const job = this.job(id); if (!job) return Promise.resolve();
    return this.run(async epoch => {
      await this.invalidate(id);
      this.job(id)!.overrideFormat = token;
      this.state.phase = 'inspecting'; await this.inspect(this.job(id)!, epoch);
    });
  }
  setPrimary(id: string, path: string) {
    const job = this.job(id); if (!job || !job.files.includes(path)) return Promise.resolve();
    return this.run(async epoch => {
      await this.invalidate(id);
      this.job(id)!.primary = path;
      this.state.phase = 'inspecting'; await this.inspect(this.job(id)!, epoch);
    });
  }
  addMissingFiles(id: string, files: File[]) {
    return this.run(async epoch => {
      const job = this.job(id); if (!job) return;
      await this.invalidate(id);
      const entries = this.inputs.get(id) ?? [];
      for (const file of files) {
        const path = file.webkitRelativePath || file.name;
        if (entries.some(entry => entry.path === path)) throw new Error(`The project already contains ${path}. Remove the case and add the corrected project folder.`);
        entries.push({ path, file });
      }
      this.inputs.set(id, entries);
      job.files = entries.map(entry => entry.path); job.size = entries.reduce((sum, entry) => sum + entry.file.size, 0);
      this.state.phase = 'inspecting'; await this.inspect(job, epoch);
    });
  }
  retry(id?: string) {
    return this.run(async epoch => {
      this.state.phase = 'inspecting';
      const ids = this.state.jobs.filter(job => id ? job.id === id : job.status === 'error' || job.status === 'cancelled' || job.status === 'needs-files' || job.outputs.some(output => output.status === 'error')).map(job => job.id);
      for (const key of ids) {
        if (epoch !== this.epoch) return;
        const job = this.job(key)!;
        if (this.inputs.has(key)) await this.inspect(job, epoch);
      }
    });
  }
  convert() {
    return this.run(async epoch => {
      const jobs = this.state.jobs.filter(job => job.family && !['error', 'needs-primary', 'needs-files'].includes(job.status));
      const selection = jobs.map(job => ({ id: job.id, targets: [...(job.targets ?? this.state.targets[job.family!])].filter(token => {
        const required = this.state.formats.find(format => format.token === token)?.requiresValueType;
        return !required || required === job.valueType;
      }) }));
      if (!selection.some(job => job.targets.length)) { this.state.message = 'Choose at least one output format.'; this.publish(); return; }
      this.state.phase = 'converting'; this.publish();
      const started = performance.now();
      track('batch_started', { count: bucket(selection.length), version: this.state.engine.version });
      for (const selected of selection) {
        if (epoch !== this.epoch) return;
        if (!selected.targets.length) continue;
        if (!await this.inspect(this.job(selected.id)!, epoch, false)) continue;
        this.job(selected.id)!.status = 'converting'; this.publish();
        for (const token of selected.targets) {
          if (epoch !== this.epoch) return;
          const job = this.job(selected.id)!;
          const existing = job.outputs.find(output => output.format === token);
          if (existing && existing.status !== 'error') continue;
          const format = this.state.formats.find(format => format.token === token)!;
          const retainedCgmesFile = token === 'cgmes' && job.format === 'cgmes' && job.primary && job.files.length === 1;
          const filename = retainedCgmesFile
            ? `${stem(job.name)}.${/\.zip$/i.test(job.primary!) ? 'zip' : 'xml'}`
            : format.isDirectory ? stem(job.name) : `${stem(job.name)}.${format.extension ?? 'json'}`;
          let result;
          let restart = false;
          try { result = await this.client!.request('emit', { format: token, name: filename }); }
          catch (error) {
            if (epoch !== this.epoch) return;
            result = { ok: false, diagnostics: [issue(message(error), 'WEB.CONVERT.WORKER')] };
            restart = true;
          }
          if (epoch !== this.epoch) return;
          const output: Output = {
            id: crypto.randomUUID(), format: token, paths: [], layout: result.layout, size: 0,
            status: !result.ok ? 'error' : result.fidelity === 'exact' ? 'unchanged' : warned([...job.diagnostics, ...result.diagnostics]) ? 'warnings' : 'converted',
            diagnostics: result.diagnostics ?? [],
          };
          const saved: SavedArtifact[] = [];
          try {
            for (const [index, artifact] of (result.artifacts ?? []).entries()) {
              const key = `${output.id}-${index}`;
              await this.store.put(key, artifact.bytes);
              saved.push({ key, path: artifact.path, size: artifact.bytes.length });
              output.paths.push(artifact.path); output.size += artifact.bytes.length;
            }
            if (epoch !== this.epoch) {
              for (const artifact of saved) await this.store.remove(artifact.key);
              return;
            }
          } catch (error) {
            for (const artifact of saved) await this.store.remove(artifact.key);
            output.status = 'error'; output.paths = []; output.size = 0;
            output.diagnostics.push(issue('Local storage is full. Download completed results and remove those cases, then retry the remaining conversion.', 'WEB.STORAGE.FULL'));
            this.job(selected.id)!.outputs = [...this.job(selected.id)!.outputs.filter(entry => entry.format !== token), output];
            this.job(selected.id)!.status = 'done'; this.publish();
            trackOperation('convert', 'error', job.format, token, output.diagnostics);
            throw error;
          }
          this.artifacts.set(output.id, saved);
          const current = this.job(selected.id)!;
          current.outputs = [...current.outputs.filter(entry => entry.format !== token), output];
          trackOperation('convert', output.status, job.format, token, output.diagnostics);
          this.publish();
          if (restart) {
            this.client?.close(); this.client = undefined;
            await this.engine();
            if (epoch !== this.epoch) return;
            if (!await this.inspect(this.job(selected.id)!, epoch, false)) break;
            this.job(selected.id)!.status = 'converting';
          }
        }
        await this.client!.request('release');
        this.job(selected.id)!.status = 'done'; this.publish();
      }
      track('batch_completed', { count: bucket(selection.length), duration: performance.now() - started < 10_000 ? 'under-10s' : '10s-plus' });
      this.state.message = 'Conversion finished. Review any warnings, then download your files.';
    });
  }
  cancel() {
    if (this.state.phase === 'converting') track('batch_cancelled');
    this.epoch++; this.client?.close(); this.client = undefined;
    this.state.jobs.forEach(job => { if (['queued', 'inspecting', 'converting'].includes(job.status)) job.status = 'cancelled'; });
    this.state.phase = 'idle'; this.state.message = 'Cancelled. Completed results are still available.'; this.publish();
  }
  async remove(id: string) {
    const job = this.job(id); if (!job) return;
    for (const output of job.outputs) {
      for (const artifact of this.artifacts.get(output.id) ?? []) await this.store.remove(artifact.key);
      this.artifacts.delete(output.id);
    }
    this.inputs.delete(id); this.state.jobs = this.state.jobs.filter(job => job.id !== id); this.publish();
  }
  private async invalidate(id: string) {
    const job = this.job(id); if (!job) return;
    for (const output of job.outputs) {
      for (const artifact of this.artifacts.get(output.id) ?? []) await this.store.remove(artifact.key);
      this.artifacts.delete(output.id);
    }
    job.outputs = [];
  }
  async clear() {
    this.cancel(); await this.chain;
    await this.store.clear(); this.inputs.clear(); this.artifacts.clear();
    this.state.jobs = []; this.state.error = ''; this.state.message = ''; this.publish();
  }
  private report() {
    return JSON.stringify({ engine: this.state.engine, cases: this.state.jobs.map(job => ({ name: job.name, format: job.format, valueType: job.valueType, status: job.status, diagnostics: job.diagnostics, outputs: job.outputs })) }, null, 2);
  }
  downloadReport() { download(new Blob([this.report()], { type: 'application/json' }), 'powerio-conversion-report.json'); }
  downloadAll() {
    return this.run(async epoch => {
      this.state.phase = 'packaging'; this.publish();
      const entries: { key: string; path: string }[] = [];
      const roots = new Set<string>();
      for (const job of this.state.jobs) {
        const root = stem(job.name);
        let count = 1; let unique = root;
        while (roots.has(unique.toLowerCase())) unique = `${root}-${++count}`;
        roots.add(unique.toLowerCase());
        for (const output of job.outputs) for (const artifact of this.artifacts.get(output.id) ?? []) {
          entries.push({ key: artifact.key, path: `${safeName(output.format)}/${unique}/${artifact.path}` });
        }
      }
      if (!entries.length) return;
      const blob = await this.store.zip(entries, this.report(), () => epoch !== this.epoch);
      if (epoch !== this.epoch) return;
      download(blob, 'powerio-converted.zip');
      track('download', { kind: 'batch', count: bucket(entries.length) });
    });
  }
  downloadOutput(jobId: string, outputId: string) {
    return this.run(async epoch => {
      const output = this.job(jobId)?.outputs.find(output => output.id === outputId);
      const files = this.artifacts.get(outputId) ?? [];
      if (!output || !files.length) return;
      if (files.length === 1 && output.layout === 'file') {
        const blob = await this.store.get(files[0].key);
        if (epoch !== this.epoch) return;
        download(blob, basename(files[0].path));
      } else {
        this.state.phase = 'packaging'; this.publish();
        const blob = await this.store.zip(files, undefined, () => epoch !== this.epoch);
        if (epoch !== this.epoch) return;
        download(blob, `${stem(this.job(jobId)!.name)}-${safeName(output.format)}.zip`);
      }
      track('download', { kind: 'single', target: output.format });
    });
  }
  issueReport(id?: string) {
    const jobs = this.state.jobs.filter(job => !id || job.id === id);
    const codes = [...new Set(jobs.flatMap(job => [...job.diagnostics, ...job.outputs.flatMap(output => output.diagnostics)]).map(entry => entry.code).filter(code => /^[A-Z][A-Z0-9_.]{1,100}$/.test(code)))];
    track('issue_opened');
    return `PowerIO conversion report\n\nEngine: ${this.state.engine.version} (${this.state.engine.commit})\nBrowser: ${browserVersion()}\nInput formats: ${[...new Set(jobs.map(job => job.format ?? 'unknown'))].join(', ')}\nOutput formats: ${[...new Set(jobs.flatMap(job => job.outputs.map(output => output.format)))].join(', ')}\nDiagnostic codes: ${codes.join(', ') || 'none'}\n\nWhat I expected:\n\nWhat happened:\n\nSteps to reproduce with a small, shareable example:\n\n`;
  }
  commands(id?: string) {
    const job = this.state.jobs.find(job => (!id || job.id === id) && job.family);
    if (!job) return 'cargo install powerio-cli --locked\npowerio convert case.raw --to matpower -o case.m';
    const tokens = job.targets ?? this.state.targets[job.family!];
    const input = job.primary ?? job.name;
    return ['cargo install powerio-cli --locked', ...tokens.map(token => {
      const format = this.state.formats.find(format => format.token === token)!;
      const output = format.isDirectory ? `${stem(job.name)}-${safeName(token)}` : `${stem(job.name)}-${safeName(token)}.${format.extension ?? 'json'}`;
      return `powerio convert ${quote(input)}${job.overrideFormat ? ` --from ${quote(job.overrideFormat)}` : ''} --to ${quote(token)} -o ${quote(output)}`;
    })].join('\n');
  }
  trackCLI() { track('cli_opened'); }
  async addExample(kind: 'transmission' | 'distribution' | 'mixed') {
    const sources = kind === 'mixed' ? ['transmission', 'distribution'] : [kind];
    const files: File[] = [];
    for (const source of sources) {
      const name = source === 'transmission' ? 'case9.m' : 'feeder.dss';
      const response = await fetch(`${import.meta.env.BASE_URL}examples/${name}`);
      if (!response.ok) { this.state.error = 'The example could not load. Choose a file or try again.'; this.publish(); return; }
      files.push(new File([await response.blob()], name));
    }
    track('example', { kind }); return this.addFiles(files);
  }
  setAnalytics(enabled: boolean) { this.state.analytics = configureAnalytics(enabled); this.publish(); }
  private applySharedSettings() {
    const { family, target } = document.documentElement.dataset;
    if ((family === 'transmission' || family === 'distribution') && this.state.formats.some(format => format.family === family && format.token === target && format.canEmit)) {
      this.state.targets[family] = [target!];
    }
    const params = new URLSearchParams(location.hash.slice(1));
    if (params.get('v') !== '1') return;
    for (const family of ['transmission', 'distribution'] as const) {
      const tokens = params.get(family)?.split(',').filter(token => this.state.formats.some(format => format.token === token && format.family === family && format.canEmit));
      if (tokens?.length) this.state.targets[family] = tokens;
    }
  }
  async share() {
    const params = new URLSearchParams({ v: '1', ...Object.fromEntries(Object.entries(this.state.targets).map(([family, tokens]) => [family, tokens.join(',')])) });
    const url = `https://powerio.dev/convert/#${params}`;
    try {
      if (navigator.share) await navigator.share({ title: 'PowerIO Convert', text: 'Convert power system files in your browser.', url });
      else { await navigator.clipboard.writeText(url); this.state.message = 'Settings link copied. It contains no files or filenames.'; this.publish(); }
      track('settings_shared');
    } catch (error) { if (!(error instanceof DOMException && error.name === 'AbortError')) { this.state.error = message(error); this.publish(); } }
  }
  dispose() { this.cancel(); this.store.dispose(); this.listeners.clear(); }
}
