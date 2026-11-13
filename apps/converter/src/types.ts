export type Family = 'transmission' | 'distribution';
export interface FormatInfo {
  token: string;
  label: string;
  family: Family;
  canRead: boolean;
  canEmit: boolean;
  isDirectory: boolean;
  extension: string | null;
  requiresValueType?: string | null;
}
export interface Diagnostic {
  code: string;
  severity: 'note' | 'remark' | 'warning' | 'error';
  message: string;
  target?: string;
  suggestedAction?: string;
  spans?: { source: string; start: number; end: number }[];
}
export interface Output {
  id: string;
  format: string;
  paths: string[];
  layout?: 'file' | 'directory';
  size: number;
  status: 'converted' | 'warnings' | 'unchanged' | 'error';
  diagnostics: Diagnostic[];
}
export interface Job {
  collectionKey?: string;
  selection?: CollectionSelection;
  id: string;
  name: string;
  files: string[];
  size: number;
  format?: string;
  family?: Family;
  valueType?: string;
  status: 'queued' | 'inspecting' | 'ready' | 'converting' | 'done' | 'error' | 'cancelled' | 'needs-primary' | 'needs-files';
  diagnostics: Diagnostic[];
  outputs: Output[];
  primary?: string;
  overrideFormat?: string;
  acquisitionFailed?: boolean;
  targets?: string[];
}
export interface ConverterState {
  formats: FormatInfo[];
  engine: { version: string; commit: string };
  jobs: Job[];
  targets: Record<Family, string[]>;
  phase: 'loading' | 'idle' | 'inspecting' | 'converting' | 'packaging';
  activeName: string;
  message: string;
  error: string;
  analytics: boolean;
}
export interface BrowserFile {
  /** Root of the transport ZIP that supplied this file. */
  collectionRoot?: string;
  path: string;
  file: File;
}

export interface CollectionSelection { root: string | null; primary: string | null; members?: string[] | null; format: string | null; operation: 'parse' | 'deserialize' }
export interface CollectionCandidate { selection: CollectionSelection; status: string }
