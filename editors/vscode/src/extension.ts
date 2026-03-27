import * as vscode from 'vscode';
import * as cp from 'child_process';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext) {
    const config = vscode.workspace.getConfiguration('kettu');
    const kettuPath = config.get<string>('path', 'kettu');

    // ─── LSP Client ────────────────────────────────────────────────────
    const serverOptions: ServerOptions = {
        run: { command: kettuPath, args: ['lsp'] },
        debug: { command: kettuPath, args: ['lsp'] },
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [
            { scheme: 'file', language: 'kettu' },
            { scheme: 'file', language: 'wit' },
        ],
    };

    client = new LanguageClient(
        'kettu',
        'Kettu Language Server',
        serverOptions,
        clientOptions
    );

    client.start();
    context.subscriptions.push({ dispose: () => client?.stop() });

    // ─── MCP Tool Implementations ──────────────────────────────────────
    // These invoke `kettu mcp` via tools/call JSON-RPC over stdio.

    const mcpToolHandler = (toolName: string) => {
        return vscode.lm.registerTool(`kettu-${toolName}`, {
            async invoke(
                options: vscode.LanguageModelToolInvocationOptions<Record<string, string>>,
                _token: vscode.CancellationToken
            ): Promise<vscode.LanguageModelToolResult> {
                const result = await callMcpTool(kettuPath, toolName, options.input);
                return new vscode.LanguageModelToolResult([
                    new vscode.LanguageModelTextPart(result),
                ]);
            },
        });
    };

    context.subscriptions.push(mcpToolHandler('check'));
    context.subscriptions.push(mcpToolHandler('docs-search'));
    context.subscriptions.push(mcpToolHandler('docs-read'));
    context.subscriptions.push(mcpToolHandler('emit-wit'));
}

export function deactivate(): Thenable<void> | undefined {
    return client?.stop();
}

/**
 * Call a single MCP tool by spawning `kettu mcp`, sending a JSON-RPC
 * tools/call request, and reading the response.
 */
function callMcpTool(
    kettuPath: string,
    toolName: string,
    args: Record<string, string>
): Promise<string> {
    return new Promise((resolve, reject) => {
        const child = cp.spawn(kettuPath, ['mcp'], { stdio: ['pipe', 'pipe', 'pipe'] });

        const request = JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            method: 'tools/call',
            params: { name: toolName, arguments: args },
        });

        let stdout = '';
        child.stdout.on('data', (data: Buffer) => {
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

        child.on('error', (err: Error) => {
            reject(new Error(`Failed to spawn kettu mcp: ${err.message}`));
        });

        child.stdin.write(request + '\n');
        child.stdin.end();
    });
}
