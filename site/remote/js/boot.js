// Before the page draws: Night or Paper, as `look.js` keeps it, so neither flashes first.
(function () {
  var choice = 'system';
  try {
    choice = localStorage.getItem('trek-remote-scheme') || 'system';
  } catch (e) {
    // The phone's own setting.
  }
  var dark = choice === 'dark' || (choice === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.setAttribute('data-scheme', dark ? 'dark' : 'light');
})();
