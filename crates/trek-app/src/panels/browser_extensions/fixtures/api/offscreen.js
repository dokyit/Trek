chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (!message || message.to !== 'offscreen') return false;
  const doc = new DOMParser().parseFromString(message.html, 'text/html');
  reply(doc.getElementById('x').textContent);
  return false;
});
