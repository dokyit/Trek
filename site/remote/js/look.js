// Night or Paper: the phone's own setting, or the one chosen in Settings (kept on this phone).
// `boot.js` sets it before the page draws, so it never flashes the other one.

const KEY = 'trek-remote-scheme';

export function scheme() {
  try {
    return localStorage.getItem(KEY) || 'system';
  } catch {
    return 'system';
  }
}

export function apply(choice = scheme()) {
  const dark = choice === 'dark' || (choice === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.dataset.scheme = dark ? 'dark' : 'light';
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute('content', dark ? '#0a0a0b' : '#ffffff');
}

export function setScheme(choice) {
  try {
    if (choice === 'system') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, choice);
  } catch {
    // Chosen for this visit.
  }
  apply(choice);
}

window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => apply());
