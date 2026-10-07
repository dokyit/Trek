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
    const params = new URLSearchParams({ path: doc.uri.fsPath });
    params.set('line', String(sel.active.line + 1));
    if (command === 'ask') {
        params.set('end', String(sel.end.line + 1));
        const text = doc.getText(sel);
        if (text) params.set('selection', text);
    }
    vscode.env.openExternal(vscode.Uri.parse(`trek://${command}?${params}`));
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
