const vscode = require('vscode');
const path = require('path');
const fs = require('fs');
const cp = require('child_process');
const { LanguageClient, TransportKind } = require('vscode-languageclient/node');
const { normalizePath, hasBreakpointInRange, getBreakpointLinesInRange } = require('./debug-breakpoints');
const { collectVisibleLocals } = require('./debug-values');

let client;
let passDecorationType;
let failDecorationType;
let coverageFullDecorationType;
let coveragePartialDecorationType;
let coverageNoneDecorationType;

function findServerPath(extensionPath) {
    const config = vscode.workspace.getConfiguration('kettu');
    const configPath = config.get('serverPath');

    // 1. Use configured path if set
    if (configPath && configPath.length > 0) {
        console.log('Kettu LSP: using configured path');
        return configPath;
    }

    // 2. Try bundled binary (platform-specific VSIX contains just bin/kettu)
    const bundledBinary = process.platform === 'win32' ? 'kettu.exe' : 'kettu';
    const bundledPath = path.join(extensionPath, 'bin', bundledBinary);
    if (fs.existsSync(bundledPath)) {
        console.log(`Kettu LSP: found bundled binary at ${bundledPath}`);
        return bundledPath;
    }

    // 3. Try relative to extension (for development - extension is in editors/vscode)
    const kettuRoot = path.resolve(extensionPath, '..', '..');
    const debugFromExt = path.join(kettuRoot, 'target', 'debug', 'kettu');
    const releaseFromExt = path.join(kettuRoot, 'target', 'release', 'kettu');

    // Prefer debug for development (faster builds, debug symbols)
    if (fs.existsSync(debugFromExt)) {
        console.log('Kettu LSP: found debug binary relative to extension');
        return debugFromExt;
    }
    if (fs.existsSync(releaseFromExt)) {
        console.log('Kettu LSP: found release binary relative to extension');
        return releaseFromExt;
    }

    // 3b. Try workspace root (for cargo workspace builds - binary is in root target/)
    // Extension is at crates/kettu-cli/editors/vscode — 4 levels up to repo root
    const workspaceRoot = path.resolve(extensionPath, '..', '..', '..', '..');
    const debugFromRoot = path.join(workspaceRoot, 'target', 'debug', 'kettu');
    const releaseFromRoot = path.join(workspaceRoot, 'target', 'release', 'kettu');

    if (fs.existsSync(debugFromRoot)) {
        console.log('Kettu LSP: found debug binary in workspace root');
        return debugFromRoot;
    }
    if (fs.existsSync(releaseFromRoot)) {
        console.log('Kettu LSP: found release binary in workspace root');
        return releaseFromRoot;
    }

    // 3. Try to find in workspace (cargo build output)
    const workspaceFolders = vscode.workspace.workspaceFolders;
    if (workspaceFolders) {
        for (const folder of workspaceFolders) {
            const releasePath = path.join(folder.uri.fsPath, 'target', 'release', 'kettu');
            const debugPath = path.join(folder.uri.fsPath, 'target', 'debug', 'kettu');

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

    // Add kettu binary directory to integrated terminal PATH via env contribution
    const binDir = path.dirname(path.resolve(serverPath));
    if (binDir && binDir !== '.') {
        const envCollection = context.environmentVariableCollection;
        envCollection.prepend('PATH', binDir + path.delimiter);
        envCollection.description = 'Adds the Kettu compiler to PATH';
    }

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

    const debugConfigProvider = new KettuDebugConfigurationProvider(serverPath);
    const debugAdapterFactory = new KettuDebugAdapterFactory(serverPath);
    context.subscriptions.push(
        vscode.debug.registerDebugConfigurationProvider('kettu', debugConfigProvider)
    );
    context.subscriptions.push(
        vscode.debug.registerDebugAdapterDescriptorFactory('kettu', debugAdapterFactory)
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('kettu.debugCurrentFileTests', async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== 'kettu') {
                vscode.window.showErrorMessage('Open a .kettu file to debug tests.');
                return;
            }

            const program = editor.document.uri.fsPath;
            const folder = vscode.workspace.getWorkspaceFolder(editor.document.uri);
            const cwd = folder ? folder.uri.fsPath : path.dirname(program);

            const config = {
                type: 'kettu',
                request: 'launch',
                name: 'Debug Kettu Tests',
                program,
                cwd,
                stopOnEntry: false,
                kettuPath: serverPath,
            };

            const ok = await vscode.debug.startDebugging(folder, config);
            if (!ok) {
                vscode.window.showErrorMessage('Failed to start Kettu debug session.');
            }
        })
    );

    // ─── MCP Tool Implementations (CoPilot Chat tools) ──────────────────
    // These invoke `kettu mcp` via JSON-RPC tools/call over stdio.

    function registerMcpTool(toolName) {
        return vscode.lm.registerTool(`kettu-${toolName}`, {
            async invoke(options, _token) {
                const result = await callMcpTool(serverPath, toolName, options.input);
                return new vscode.LanguageModelToolResult([
                    new vscode.LanguageModelTextPart(result),
                ]);
            },
        });
    }

    context.subscriptions.push(registerMcpTool('check'));
    context.subscriptions.push(registerMcpTool('docs-search'));
    context.subscriptions.push(registerMcpTool('docs-read'));
    context.subscriptions.push(registerMcpTool('emit-wit'));

    // ─── MCP Server Definition Provider (CoPilot Agent Mode) ────────────
    // This makes "kettu mcp" appear in the MCP server list.

    const didChangeEmitter = new vscode.EventEmitter();
    context.subscriptions.push(
        vscode.lm.registerMcpServerDefinitionProvider('kettuMcpProvider', {
            onDidChangeMcpServerDefinitions: didChangeEmitter.event,
            provideMcpServerDefinitions: async () => {
                return [
                    new vscode.McpStdioServerDefinition(
                        'Kettu',
                        serverPath,
                        ['mcp'],
                    ),
                ];
            },
            resolveMcpServerDefinition: async (server) => server,
        })
    );
}

class KettuDebugConfigurationProvider {
    constructor(defaultKettuPath) {
        this.defaultKettuPath = defaultKettuPath;
    }

    resolveDebugConfiguration(_folder, config) {
        if (!config.type) {
            config.type = 'kettu';
        }
        if (!config.request) {
            config.request = 'launch';
        }

        if (!config.program) {
            const editor = vscode.window.activeTextEditor;
            if (editor && editor.document.languageId === 'kettu') {
                config.program = editor.document.uri.fsPath;
            }
        }

        if (!config.program) {
            vscode.window.showErrorMessage('Kettu debug: set a .kettu file in launch.json "program".');
            return undefined;
        }

        config.cwd = config.cwd || path.dirname(config.program);
        config.kettuPath = config.kettuPath || this.defaultKettuPath;
        config.stopOnEntry = !!config.stopOnEntry;
        return config;
    }
}

class KettuDebugAdapterFactory {
    constructor(defaultKettuPath) {
        this.defaultKettuPath = defaultKettuPath;
    }

    createDebugAdapterDescriptor(session) {
        const adapter = new KettuInlineDebugAdapter(session.configuration, this.defaultKettuPath);
        return new vscode.DebugAdapterInlineImplementation(adapter);
    }
}

class KettuInlineDebugAdapter {
    constructor(configuration, defaultKettuPath) {
        this.configuration = configuration || {};
        this.defaultKettuPath = defaultKettuPath;
        this.eventEmitter = new vscode.EventEmitter();
        this.onDidSendMessage = this.eventEmitter.event;

        this.breakpoints = new Map();
        this.currentStop = null;
        this.started = false;
        this.terminated = false;
        this.continueResolver = null;
        this.resumeAction = 'continue';
        this.threadId = 1;
        this.contextVariablesRef = 1;
        this.localsVariablesRef = 2;
        this.sourceText = '';
    }

    dispose() {
        this.terminated = true;
        if (this.continueResolver) {
            this.continueResolver();
            this.continueResolver = null;
        }
        this.eventEmitter.dispose();
    }

    async handleMessage(message) {
        if (message.type !== 'request') {
            return;
        }

        switch (message.command) {
            case 'initialize': {
                this.sendResponse(message, {
                    supportsConfigurationDoneRequest: true,
                    supportsConditionalBreakpoints: false,
                    supportsLogPoints: false,
                    supportsEvaluateForHovers: false,
                    supportsSetVariable: false,
                    supportsTerminateRequest: true,
                });
                break;
            }

            case 'launch': {
                const args = message.arguments || {};
                this.program = path.resolve(args.program || this.configuration.program || '');
                this.cwd = path.resolve(args.cwd || this.configuration.cwd || path.dirname(this.program));
                this.kettuPath = args.kettuPath || this.configuration.kettuPath || this.defaultKettuPath;
                this.stopOnEntry = !!(args.stopOnEntry ?? this.configuration.stopOnEntry);

                if (!this.program) {
                    this.sendErrorResponse(message, 'Missing launch argument: program');
                    return;
                }

                this.sendResponse(message);
                this.sendEvent('initialized');
                break;
            }

            case 'setBreakpoints': {
                const sourcePath = message.arguments?.source?.path;
                const resolved = sourcePath ? normalizePath(sourcePath) : normalizePath(this.program);
                const lines = (message.arguments?.breakpoints || [])
                    .map((bp) => bp.line)
                    .filter((line) => Number.isInteger(line));

                this.breakpoints.set(resolved, new Set(lines));

                this.sendResponse(message, {
                    breakpoints: lines.map((line) => ({
                        verified: true,
                        line,
                    })),
                });
                break;
            }

            case 'configurationDone': {
                this.sendResponse(message);
                this.startExecution().catch((err) => {
                    this.sendOutput(`Kettu debug error: ${err.message}\n`, 'stderr');
                    this.sendEvent('terminated');
                });
                break;
            }

            case 'threads': {
                this.sendResponse(message, {
                    threads: [{ id: this.threadId, name: 'Kettu Tests' }],
                });
                break;
            }

            case 'stackTrace': {
                if (!this.currentStop) {
                    this.sendResponse(message, { stackFrames: [], totalFrames: 0 });
                    break;
                }

                this.sendResponse(message, {
                    stackFrames: [
                        {
                            id: 1,
                            name: `test ${this.currentStop.name}`,
                            source: {
                                name: path.basename(this.program),
                                path: this.program,
                            },
                            line: this.currentStop.line,
                            column: 1,
                        },
                    ],
                    totalFrames: 1,
                });
                break;
            }

            case 'scopes': {
                this.sendResponse(message, {
                    scopes: [
                        {
                            name: 'Locals',
                            variablesReference: this.localsVariablesRef,
                            expensive: false,
                        },
                        {
                            name: 'Context',
                            variablesReference: this.contextVariablesRef,
                            expensive: false,
                        },
                    ],
                });
                break;
            }

            case 'variables': {
                const vars = [];
                if (this.currentStop) {
                    if (message.arguments?.variablesReference === this.localsVariablesRef) {
                        const locals = this.currentStop.locals || {};
                        for (const [name, value] of Object.entries(locals)) {
                            vars.push({
                                name,
                                value: String(value),
                                type: typeof value,
                                variablesReference: 0,
                            });
                        }
                    } else {
                        vars.push({ name: 'test', value: this.currentStop.name, variablesReference: 0 });
                        vars.push({ name: 'line', value: String(this.currentStop.line), variablesReference: 0 });
                        vars.push({ name: 'status', value: this.currentStop.status, variablesReference: 0 });
                    }
                }
                this.sendResponse(message, { variables: vars });
                break;
            }

            case 'continue': {
                this.resumeAction = 'continue';
                if (this.continueResolver) {
                    this.continueResolver(this.resumeAction);
                    this.continueResolver = null;
                }
                this.sendResponse(message, { allThreadsContinued: true });
                break;
            }

            case 'next':
            case 'stepIn':
            case 'stepOut': {
                this.resumeAction = message.command;
                if (this.continueResolver) {
                    this.continueResolver(this.resumeAction);
                    this.continueResolver = null;
                }
                this.sendResponse(message);
                break;
            }

            case 'disconnect':
            case 'terminate': {
                this.terminated = true;
                this.resumeAction = 'terminate';
                if (this.continueResolver) {
                    this.continueResolver(this.resumeAction);
                    this.continueResolver = null;
                }
                this.sendResponse(message);
                this.sendEvent('terminated');
                break;
            }

            default: {
                this.sendResponse(message);
            }
        }
    }

    async startExecution() {
        if (this.started || this.terminated) {
            return;
        }
        this.started = true;

        try {
            this.sourceText = fs.readFileSync(this.program, 'utf8');
        } catch {
            this.sourceText = '';
        }

        const discovery = await runCommandJson(this.kettuPath, ['test', this.program, '--list', '--json'], this.cwd);
        const tests = Array.isArray(discovery.tests) ? discovery.tests : [];

        if (tests.length === 0) {
            this.sendOutput('No tests found.\n');
            this.sendEvent('terminated');
            return;
        }

        if (this.stopOnEntry) {
            const entryLine = Number.isInteger(tests[0].line) ? tests[0].line : 1;
            const locals = this.collectLocalsForStop(entryLine, entryLine);
            await this.stopAndWait('entry', { name: tests[0].name, line: entryLine, status: 'ready', locals });
        }

        for (const test of tests) {
            if (this.terminated) {
                break;
            }

            const startLine = Number.isInteger(test.line) ? test.line : 1;
            const endLine = Number.isInteger(test.endLine) ? test.endLine : startLine;

            const breakpointLines = this.getBreakpointLines(this.program, startLine, endLine);
            if (breakpointLines.length > 0) {
                let stopLine = breakpointLines[0];
                let action = await this.stopAndWait('breakpoint', {
                    name: test.name,
                    line: stopLine,
                    status: 'paused',
                    locals: this.collectLocalsForStop(startLine, stopLine),
                });

                while (!this.terminated && (action === 'next' || action === 'stepIn' || action === 'stepOut')) {
                    const nextBreakpoint = breakpointLines.find((line) => line > stopLine);
                    if (typeof nextBreakpoint === 'number') {
                        stopLine = nextBreakpoint;
                    } else if (stopLine < endLine) {
                        stopLine = Math.min(stopLine + 1, endLine);
                    } else {
                        break;
                    }

                    action = await this.stopAndWait('step', {
                        name: test.name,
                        line: stopLine,
                        status: 'stepping',
                        locals: this.collectLocalsForStop(startLine, stopLine),
                    });
                }
            }

            if (this.terminated) {
                break;
            }

            const runResult = await runCommand(
                this.kettuPath,
                ['test', this.program, '--filter', test.name, '--exact'],
                this.cwd
            );

            if (runResult.stdout) {
                this.sendOutput(runResult.stdout);
            }
            if (runResult.stderr) {
                this.sendOutput(runResult.stderr, 'stderr');
            }

            if (runResult.exitCode !== 0) {
                await this.stopAndWait('exception', {
                    name: test.name,
                    line: startLine,
                    status: 'failed',
                    locals: this.collectLocalsForStop(startLine, startLine),
                });
            }
        }

        if (!this.terminated) {
            this.sendEvent('terminated');
        }
    }

    hasBreakpoint(filePath, startLine, endLine) {
        return hasBreakpointInRange(this.breakpoints, filePath, startLine, endLine);
    }

    getBreakpointLines(filePath, startLine, endLine) {
        return getBreakpointLinesInRange(this.breakpoints, filePath, startLine, endLine);
    }

    collectLocalsForStop(startLine, stopLine) {
        if (!this.sourceText) {
            return {};
        }
        return collectVisibleLocals(this.sourceText, startLine, stopLine);
    }

    async stopAndWait(reason, stop) {
        this.currentStop = stop;
        this.sendEvent('stopped', {
            reason,
            threadId: this.threadId,
            allThreadsStopped: true,
            description: `${stop.name} (${stop.status})`,
        });

        this.resumeAction = 'continue';
        const action = await new Promise((resolve) => {
            this.continueResolver = resolve;
        });
        return action || 'continue';
    }

    sendEvent(event, body = {}) {
        this.eventEmitter.fire({
            type: 'event',
            event,
            body,
        });
    }

    sendResponse(request, body = {}) {
        this.eventEmitter.fire({
            type: 'response',
            seq: 0,
            request_seq: request.seq,
            success: true,
            command: request.command,
            body,
        });
    }

    sendErrorResponse(request, message) {
        this.eventEmitter.fire({
            type: 'response',
            seq: 0,
            request_seq: request.seq,
            success: false,
            command: request.command,
            message,
            body: {
                error: {
                    id: 1,
                    format: message,
                },
            },
        });
    }

    sendOutput(output, category = 'stdout') {
        this.sendEvent('output', {
            category,
            output,
        });
    }
}

function runCommand(command, args, cwd) {
    return new Promise((resolve, reject) => {
        const child = cp.spawn(command, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'] });
        let stdout = '';
        let stderr = '';

        child.stdout.on('data', (chunk) => {
            stdout += chunk.toString();
        });
        child.stderr.on('data', (chunk) => {
            stderr += chunk.toString();
        });

        child.on('error', (err) => {
            reject(err);
        });

        child.on('close', (code) => {
            resolve({
                exitCode: code ?? 1,
                stdout,
                stderr,
            });
        });
    });
}

async function runCommandJson(command, args, cwd) {
    const result = await runCommand(command, args, cwd);
    if (result.exitCode !== 0) {
        throw new Error(result.stderr || result.stdout || `Command failed: ${command} ${args.join(' ')}`);
    }

    const text = (result.stdout || '').trim();
    if (!text) {
        throw new Error('Expected JSON output, got empty output');
    }

    try {
        return JSON.parse(text);
    } catch (err) {
        throw new Error(`Failed to parse JSON output: ${err.message}\n${text}`);
    }
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

/**
 * Call a single MCP tool by spawning `kettu mcp`, sending a JSON-RPC
 * tools/call request, and reading the response.
 */
function callMcpTool(kettuPath, toolName, args) {
    return new Promise((resolve, reject) => {
        const child = cp.spawn(kettuPath, ['mcp'], { stdio: ['pipe', 'pipe', 'pipe'] });

        const request = JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            method: 'tools/call',
            params: { name: toolName, arguments: args },
        });

        let stdout = '';
        child.stdout.on('data', (data) => {
            stdout += data.toString();
        });

        child.on('close', () => {
            try {
                const response = JSON.parse(stdout.trim());
                if (response.result?.content?.[0]?.text) {
                    resolve(response.result.content[0].text);
                } else if (response.error) {
                    reject(new Error(response.error.message));
                } else {
                    resolve(stdout.trim());
                }
            } catch {
                reject(new Error(`Failed to parse MCP response: ${stdout}`));
            }
        });

        child.on('error', (err) => {
            reject(new Error(`Failed to spawn kettu mcp: ${err.message}`));
        });

        child.stdin.write(request + '\n');
        child.stdin.end();
    });
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
