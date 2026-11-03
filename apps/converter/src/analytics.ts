import type { Diagnostic } from './types';

const policy = globalThis.powerioAnalyticsPolicy;
const withinBudget = policy?.createBudget() ?? (() => false);
let frame: HTMLIFrameElement | undefined;
let ready = false;
let queued: UsageEvent[] = [];
const privacySignal = () => navigator.doNotTrack === '1'
  || (navigator as Navigator & { globalPrivacyControl?: boolean }).globalPrivacyControl === true;

function send(event: UsageEvent) {
  if (!privacySignal()) frame?.contentWindow?.postMessage({ type: 'powerio-usage', ...event }, '*');
}
window.addEventListener('message', event => {
  if (!frame || event.source !== frame.contentWindow || event.data?.type !== 'powerio-usage-ready') return;
  ready = true;
  queued.forEach(send);
  queued = [];
});
export function analyticsEnabled() {
  try { return localStorage.getItem('powerio-analytics') === 'on' && !privacySignal(); }
  catch { return false; }
}
export function configureAnalytics(enabled: boolean) {
  const permitted = enabled && !!policy && !privacySignal();
  try { localStorage.setItem('powerio-analytics', permitted ? 'on' : 'off'); } catch { /* Browser settings can disable preference storage. */ }
  frame?.remove(); frame = undefined; ready = false; queued = [];
  if (!permitted || location.hostname !== 'powerio.dev') return permitted;
  frame = document.createElement('iframe');
  frame.hidden = true;
  frame.title = 'Optional usage and error statistics';
  frame.sandbox.add('allow-scripts');
  frame.src = `${import.meta.env.BASE_URL}analytics.html`;
  const activeFrame = frame;
  frame.addEventListener('load', () => {
    if (frame === activeFrame && !privacySignal()) frame.contentWindow?.postMessage({ type: 'powerio-usage-start' }, '*');
  }, { once: true });
  document.body.append(frame);
  return permitted;
}
export function track(name: string, data: Record<string, string> = {}) {
  if (!frame || !policy || privacySignal()) return;
  const event = policy.normalize(name, data);
  if (!event || !withinBudget(event)) return;
  if (ready) send(event); else queued.push(event);
}
export function trackOperation(stage: 'input' | 'parse' | 'convert', outcome: string, source?: string, target?: string, diagnostics: Diagnostic[] = []) {
  if (!frame || !policy || privacySignal()) return;
  const formats = { source: policy.safeFormat(source), ...(target ? { target: policy.safeFormat(target) } : {}) };
  track(`${stage}_result`, { ...formats, outcome });
  // Only code identities enter the analytics path; messages and electrical details stay local.
  const codes = [...new Set(diagnostics.map(item => policy.safeCode(item.code)))].sort();
  for (const code of codes.slice(0, 3)) track('problem', { ...formats, stage, code });
}
export function bucket(count: number) { return count < 2 ? '1' : count < 11 ? '2-10' : count < 101 ? '11-100' : '101-plus'; }
