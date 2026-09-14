const policy = globalThis.powerioAnalyticsPolicy;
const withinBudget = policy?.createBudget() ?? (() => false);
const base = { website: '48166729-f7e7-4022-8eb7-f57d95133755', hostname: 'powerio.dev', url: '/convert/', title: 'PowerIO Convert' };
const permitted = () => navigator.doNotTrack !== '1' && navigator.globalPrivacyControl !== true;
let started = false;
window.addEventListener('message', event => {
  if (!policy || !permitted() || event.source !== parent || event.origin !== 'https://powerio.dev' || parent === window) return;
  if (event.data?.type === 'powerio-usage-start' && !started) {
    started = true;
    const script = document.createElement('script');
    script.src = 'https://cloud.umami.is/script.js';
    script.defer = true;
    script.dataset.websiteId = base.website;
    script.dataset.autoTrack = 'false';
    script.dataset.hostUrl = 'https://gateway.umami.is';
    script.dataset.doNotTrack = 'true';
    script.dataset.excludeSearch = 'true';
    script.dataset.excludeHash = 'true';
    script.addEventListener('load', () => {
      if (!window.umami || !permitted()) return;
      window.umami.track(base);
      parent.postMessage({ type: 'powerio-usage-ready' }, 'https://powerio.dev');
    }, { once: true });
    document.head.append(script);
  } else if (event.data?.type === 'powerio-usage' && started) {
    const safe = policy.normalize(event.data.name, event.data.data);
    if (safe && withinBudget(safe)) window.umami?.track({ ...base, ...safe });
  }
});
