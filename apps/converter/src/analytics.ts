const allowed = new Set(['example', 'batch_started', 'batch_completed', 'download', 'settings_shared', 'cli_opened', 'issue_opened', 'conversion_error']);
const properties = new Set(['kind', 'source', 'target', 'family', 'count', 'duration', 'outcome', 'code', 'version']);
let frame: HTMLIFrameElement | undefined;
let ready = false;
let queued: { name: string; data: Record<string, string> }[] = [];

function send(event: { name: string; data: Record<string, string> }) {
  frame?.contentWindow?.postMessage({ type: 'powerio-usage', ...event }, '*');
}
window.addEventListener('message', event => {
  if (event.source !== frame?.contentWindow || event.data?.type !== 'powerio-usage-ready') return;
  ready = true;
  queued.forEach(send);
  queued = [];
});
export function analyticsEnabled() {
  try { return localStorage.getItem('powerio-analytics') !== 'off' && navigator.doNotTrack !== '1'; }
  catch { return false; }
}
export function configureAnalytics(enabled: boolean) {
  try { localStorage.setItem('powerio-analytics', enabled ? 'on' : 'off'); } catch { /* Browser settings can disable preference storage. */ }
  frame?.remove(); frame = undefined; ready = false; queued = [];
  if (!enabled || navigator.doNotTrack === '1' || location.hostname !== 'powerio.dev') return;
  frame = document.createElement('iframe');
  frame.hidden = true;
  frame.title = 'Anonymous usage analytics';
  frame.sandbox.add('allow-scripts');
  frame.src = `${import.meta.env.BASE_URL}analytics.html`;
  document.body.append(frame);
}
export function track(name: string, data: Record<string, string> = {}) {
  if (!frame || !allowed.has(name)) return;
  const safe = Object.fromEntries(Object.entries(data).filter(([key, value]) => properties.has(key) && /^[a-zA-Z0-9_.@:-]{1,80}$/.test(value)));
  const event = { name, data: safe };
  if (ready) send(event); else if (queued.length < 20) queued.push(event);
}
export function bucket(count: number) { return count < 2 ? '1' : count < 11 ? '2-10' : count < 101 ? '11-100' : '101-plus'; }
