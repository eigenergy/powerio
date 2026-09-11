const events = new Set(['example', 'batch_started', 'batch_completed', 'download', 'settings_shared', 'cli_opened', 'issue_opened', 'conversion_error']);
const keys = new Set(['kind', 'source', 'target', 'family', 'count', 'duration', 'outcome', 'code', 'version']);
const base = { website: '48166729-f7e7-4022-8eb7-f57d95133755', hostname: 'powerio.dev', url: '/convert/', title: 'PowerIO Convert' };
window.addEventListener('load', () => {
  if (!window.umami || navigator.doNotTrack === '1') return;
  window.umami.track(base);
  parent.postMessage({ type: 'powerio-usage-ready' }, 'https://powerio.dev');
});
window.addEventListener('message', event => {
  if (event.source !== parent || event.origin !== 'https://powerio.dev' || event.data?.type !== 'powerio-usage' || !events.has(event.data.name)) return;
  const data = Object.fromEntries(Object.entries(event.data.data ?? {}).filter(([key, value]) => keys.has(key) && typeof value === 'string' && /^[a-zA-Z0-9_.@:-]{1,80}$/.test(value)));
  window.umami?.track({ ...base, name: event.data.name, data });
});
