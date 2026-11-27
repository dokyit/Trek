// The driver's `ext-eval` runs `run('<command>', arg)` here: the background's answer.
window.run = (command, arg) => chrome.runtime.sendMessage({ command, arg });
