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
  size: number;
  status: 'converted' | 'warnings' | 'unchanged' | 'error';
  diagnostics: Diagnostic[];
}
export interface Job {
  id: string;
  name: string;
  files: string[];
  size: number;
  format?: string;
  family?: Family;
  valueType?: string;
  status: 'queued' | 'inspecting' | 'ready' | 'converting' | 'done' | 'error' | 'cancelled' | 'needs-primary';
  diagnostics: Diagnostic[];
  outputs: Output[];
  primary?: string;
  overrideFormat?: string;
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
  path: string;
  file: File;
}
