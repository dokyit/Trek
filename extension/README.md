# Trek for editors

Jump between your editor and Trek. Works in Cursor, VS Code and Windsurf.

- **Open in Trek** (`⌃⌥T`, editor title, right-click) opens the current file in Trek's
  built-in editor at the line you're on.
- **Ask Trek about selection** (right-click with a selection) starts a Trek draft on this
  project with the selection quoted into the composer.

Requires the Trek app; it rides the `trek://` URL scheme.

## Try it

```sh
cd extension && npm install -g @vscode/vsce && vsce package
```

Then `Extensions: Install from VSIX…` in your editor and pick the `.vsix` — or press F5 in an
extension-development host window.
