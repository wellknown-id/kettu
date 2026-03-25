const vscode = require('vscode');
const path = require('path');
const fs = require('fs');
const { LanguageClient, TransportKind } = require('vscode-languageclient/node');

let client;
let passDecorationType;
let failDecorationType;
let coverageFullDecorationType;
let coveragePartialDecorationType;
let coverageNoneDecorationType;

function executableName(targetPlatform) {
    if (targetPlatform) {
        return targetPlatform.startsWith('win32-') ? 'kettu.exe' : 'kettu';
    }

    return process.platform === 'win32' ? 'kettu.exe' : 'kettu';
}

function bundledTargetPlatform() {
    const byHost = {
        'darwin-arm64': 'darwin-arm64',
        'darwin-x64': 'darwin-x64',
        'linux-arm64': 'linux-arm64',
        'linux-x64': 'linux-x64',
        'win32-arm64': 'win32-arm64',
        'win32-x64': 'win32-x64',
    };

    return byHost[`${process.platform}-${process.arch}`];
}

function findServerPath(extensionPath) {
    const config = vscode.workspace.getConfiguration('kettu');
    const configPath = config.get('serverPath');
    const binaryName = executableName(bundledTargetPlatform());

    // 1. Use configured path if set
    if (configPath && configPath.length > 0) {
        console.log('Kettu LSP: using configured path');
        return configPath;
    }

    const targetPlatform = bundledTargetPlatform();
    if (targetPlatform) {
        const bundledPath = path.join(extensionPath, 'bin', targetPlatform, executableName(targetPlatform));
        if (fs.existsSync(bundledPath)) {
            console.log('Kettu LSP: using bundled compiler');
            return bundledPath;
        }
    }

    // 2. Try relative to extension (for development - extension is in editors/vscode)
    const crateRoot = path.resolve(extensionPath, '..', '..');
    const workspaceRoot = path.resolve(extensionPath, '..', '..', '..', '..');
    const debugFromCrate = path.join(crateRoot, 'target', 'debug', binaryName);
    const releaseFromCrate = path.join(crateRoot, 'target', 'release', binaryName);
    const debugFromWorkspace = path.join(workspaceRoot, 'target', 'debug', binaryName);
    const releaseFromWorkspace = path.join(workspaceRoot, 'target', 'release', binaryName);

    // Prefer debug for development (faster builds, debug symbols)
    if (fs.existsSync(debugFromCrate)) {
        console.log('Kettu LSP: found debug binary relative to extension');
        return debugFromCrate;
    }
    if (fs.existsSync(releaseFromCrate)) {
        console.log('Kettu LSP: found release binary relative to extension');
        return releaseFromCrate;
    }

    if (fs.existsSync(debugFromWorkspace)) {
        console.log('Kettu LSP: found debug binary in workspace root');
        return debugFromWorkspace;
    }
    if (fs.existsSync(releaseFromWorkspace)) {
        console.log('Kettu LSP: found release binary in workspace root');
        return releaseFromWorkspace;
    }

    // 3. Try to find in workspace (cargo build output)
    const workspaceFolders = vscode.workspace.workspaceFolders;
    if (workspaceFolders) {
        for (const folder of workspaceFolders) {
            const releasePath = path.join(folder.uri.fsPath, 'target', 'release', binaryName);
            const debugPath = path.join(folder.uri.fsPath, 'target', 'debug', binaryName);

            if (fs.existsSync(releasePath)) {
                console.log('Kettu LSP: found release binary in workspace');
                return releasePath;
            }
            if (fs.existsSync(debugPath)) {
                console.log('Kettu LSP: found debug binary in workspace');
                return debugPath;
            }
        }
    }

    // 4. Fall back to PATH
    console.log('Kettu LSP: falling back to PATH');
    return 'kettu';
}

function activate(context) {
    const serverPath = findServerPath(context.extensionPath);

    console.log(`Kettu LSP: using server at ${serverPath}`);

    // Gutter decorations for test pass/fail
    passDecorationType = vscode.window.createTextEditorDecorationType({
        gutterIconPath: path.join(context.extensionPath, 'icons', 'test-pass.svg'),
        gutterIconSize: '80%',
        overviewRulerColor: '#4ec966',
        overviewRulerLane: vscode.OverviewRulerLane.Left,
    });

    failDecorationType = vscode.window.createTextEditorDecorationType({
        gutterIconPath: path.join(context.extensionPath, 'icons', 'test-fail.svg'),
        gutterIconSize: '80%',
        overviewRulerColor: '#f14c4c',
        overviewRulerLane: vscode.OverviewRulerLane.Left,
    });

    // Gutter decorations for coverage
    coverageFullDecorationType = vscode.window.createTextEditorDecorationType({
        gutterIconPath: path.join(context.extensionPath, 'icons', 'coverage-full.svg'),
        gutterIconSize: '60%',
    });

    coveragePartialDecorationType = vscode.window.createTextEditorDecorationType({
        gutterIconPath: path.join(context.extensionPath, 'icons', 'coverage-partial.svg'),
        gutterIconSize: '60%',
    });

    coverageNoneDecorationType = vscode.window.createTextEditorDecorationType({
        gutterIconPath: path.join(context.extensionPath, 'icons', 'coverage-none.svg'),
        gutterIconSize: '60%',
    });

    // The kettu binary uses a subcommand for the LSP: `kettu lsp`
    const serverOptions = {
        run: { command: serverPath, args: ['lsp'], transport: TransportKind.stdio },
        debug: { command: serverPath, args: ['lsp'], transport: TransportKind.stdio }
    };

    const clientOptions = {
        documentSelector: [
            { scheme: 'file', language: 'kettu' },
            { scheme: 'file', language: 'wit' }
        ],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher('**/*.{kettu,wit}')
        }
    };

    client = new LanguageClient(
        'kettuLanguageServer',
        'Kettu Language Server',
        serverOptions,
        clientOptions
    );

    client.start().then(() => {
        // Listen for test results + coverage (diagnostics handled by LSP directly)
        client.onNotification('kettu/testResults', (params) => {
            applyTestDecorations(params.uri, params.tests);
            applyCoverageDecorations(params.uri, params.coverage);
        });
    }).catch(err => {
        vscode.window.showErrorMessage(
            `Failed to start Kettu LSP: ${err.message}. ` +
            `Build with 'cargo build --bin kettu' or set 'kettu.serverPath'.`
        );
    });
}

/**
 * Apply gutter decorations for test results (checkmark/X).
 */
function applyTestDecorations(uriString, results) {
    const uri = vscode.Uri.parse(uriString);
    const editor = vscode.window.visibleTextEditors.find(
        e => e.document.uri.toString() === uri.toString()
    );
    if (!editor) return;

    const passDecorations = [];
    const failDecorations = [];

    for (const result of results) {
        const range = new vscode.Range(result.line, 0, result.line, 0);
        if (result.passed) {
            passDecorations.push({ range });
        } else {
            failDecorations.push({ range });
        }
    }

    editor.setDecorations(passDecorationType, passDecorations);
    editor.setDecorations(failDecorationType, failDecorations);
}

/**
 * Apply gutter dots for coverage.
 */
function applyCoverageDecorations(uriString, coverage) {
    const uri = vscode.Uri.parse(uriString);
    const editor = vscode.window.visibleTextEditors.find(
        e => e.document.uri.toString() === uri.toString()
    );
    if (!editor) return;

    const full = [];
    const partial = [];
    const none = [];

    for (const item of coverage) {
        const range = new vscode.Range(item.line, 0, item.line, 0);

        if (item.status === 'full') {
            full.push({ range });
        } else if (item.status === 'partial') {
            partial.push({ range });
        } else {
            none.push({ range });
        }
    }

    editor.setDecorations(coverageFullDecorationType, full);
    editor.setDecorations(coveragePartialDecorationType, partial);
    editor.setDecorations(coverageNoneDecorationType, none);
}

function deactivate() {
    if (passDecorationType) passDecorationType.dispose();
    if (failDecorationType) failDecorationType.dispose();
    if (coverageFullDecorationType) coverageFullDecorationType.dispose();
    if (coveragePartialDecorationType) coveragePartialDecorationType.dispose();
    if (coverageNoneDecorationType) coverageNoneDecorationType.dispose();
    if (client) {
        return client.stop();
    }
}

module.exports = { activate, deactivate };
