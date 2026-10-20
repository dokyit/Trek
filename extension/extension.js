// Trek's editor bridge: open the file you're on — or the selection you made — in Trek.
// Cursor, VS Code and Windsurf share this extension host, so one build covers them.

const vscode = require('vscode');

function openLink(command) {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.document.uri.scheme !== 'file') {
        vscode.window.showInformationMessage('Open a file first.');
        return;
    }
    const doc = editor.document;
    const sel = editor.selection;
    // Asking about a selection names where it starts and ends, whichever way it was dragged;
    // opening the file goes to the caret.
    const params = [['path', doc.uri.fsPath], ['line', String((command === 'ask' ? sel.start : sel.active).line + 1)]];
    if (command === 'ask') {
        params.push(['end', String(sel.end.line + 1)]);
        const text = doc.getText(sel);
        if (text) params.push(['selection', text]);
    }
    // A string, not a Uri: openExternal re-encodes a Uri with encodeURI, which leaves '&', '+' and
    // '=' bare, and they'd split or blank out the text. Spaces are %20: '+' is a literal plus.
    const query = params.map(([k, v]) => `${k}=${encodeURIComponent(v)}`).join('&');
    vscode.env.openExternal(`trek://${command}?${query}`);
}

function activate(context) {
    context.subscriptions.push(
        vscode.commands.registerCommand('trek.openInTrek', () => openLink('edit')),
        vscode.commands.registerCommand('trek.askAboutSelection', () => openLink('ask')),
    );

    const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    status.text = '$(compass) Trek';
    status.tooltip = 'Open this file in Trek';
    status.command = 'trek.openInTrek';
    status.show();
    context.subscriptions.push(status);
}

function deactivate() {}

module.exports = { activate, deactivate };
