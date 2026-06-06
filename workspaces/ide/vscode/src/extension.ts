/**
 * Incan Language Extension for VS Code / Cursor
 * 
 * Provides:
 * - Syntax highlighting (via TextMate grammar)
 * - LSP integration for real-time diagnostics, hover, and go-to-definition
 * - Run/Check commands for Incan files
 */

import * as path from 'path';
import * as vscode from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    TransportKind,
} from 'vscode-languageclient/node';
import {
    BinaryResolution,
    formatDoctorReport,
    formatResolution,
    resolveBinary,
} from './binaryResolution';

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel;

function workspaceFolders(): string[] {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || folders.length === 0) {
        return [];
    }
    return folders.map(folder => folder.uri.fsPath);
}

function resolveConfiguredBinary(binaryName: string, settingName: 'compiler.path' | 'lsp.path'): BinaryResolution {
    const config = vscode.workspace.getConfiguration('incan');
    const configuredPath = config.get<string>(settingName, '').trim();
    const settingKey = `incan.${settingName}`;
    return resolveBinary({
        binaryName,
        settingKey,
        configuredPath,
        workspaceFolders: workspaceFolders(),
    });
}

function getCompilerResolution(): BinaryResolution {
    return resolveConfiguredBinary('incan', 'compiler.path');
}

function getLspResolution(): BinaryResolution {
    return resolveConfiguredBinary('incan-lsp', 'lsp.path');
}

function shellQuote(value: string): string {
    return `"${value.replace(/(["\\$`])/g, '\\$1')}"`;
}

function logResolution(resolution: BinaryResolution) {
    for (const line of formatResolution(resolution)) {
        outputChannel.appendLine(line);
    }
}

function warnForResolution(resolution: BinaryResolution) {
    if (resolution.warnings.length === 0) {
        return;
    }
    vscode.window.showWarningMessage(`Incan ${resolution.name} setup issue: ${resolution.warnings[0]}`);
}

function writeDoctorReport() {
    const compiler = getCompilerResolution();
    const lsp = getLspResolution();
    for (const line of formatDoctorReport(compiler, lsp)) {
        outputChannel.appendLine(line);
    }
}

async function showDoctor() {
    outputChannel.clear();
    writeDoctorReport();
    outputChannel.show(true);
}

function getFileToRun(uri?: vscode.Uri): string | undefined {
    // If URI provided (from explorer context menu), use it
    if (uri) {
        return uri.fsPath;
    }
    // Otherwise use active editor
    const editor = vscode.window.activeTextEditor;
    if (editor && (editor.document.languageId === 'incan' || 
                   editor.document.fileName.endsWith('.incn') ||
                   editor.document.fileName.endsWith('.incan'))) {
        return editor.document.fileName;
    }
    return undefined;
}

async function runIncanFile(uri?: vscode.Uri) {
    const filePath = getFileToRun(uri);
    if (!filePath) {
        vscode.window.showErrorMessage('No Incan file to run. Open an .incn file first.');
        return;
    }

    // Save the file before running
    const doc = vscode.workspace.textDocuments.find(d => d.fileName === filePath);
    if (doc?.isDirty) {
        await doc.save();
    }

    const compiler = getCompilerResolution();
    warnForResolution(compiler);
    const terminal = vscode.window.createTerminal({
        name: `Incan: ${path.basename(filePath)}`,
        cwd: path.dirname(filePath),
    });
    
    terminal.show();
    terminal.sendText(`${shellQuote(compiler.command)} run ${shellQuote(filePath)}`);
}

async function checkIncanFile(uri?: vscode.Uri) {
    const filePath = getFileToRun(uri);
    if (!filePath) {
        vscode.window.showErrorMessage('No Incan file to check. Open an .incn file first.');
        return;
    }

    // Save the file before checking
    const doc = vscode.workspace.textDocuments.find(d => d.fileName === filePath);
    if (doc?.isDirty) {
        await doc.save();
    }

    const compiler = getCompilerResolution();
    warnForResolution(compiler);
    const terminal = vscode.window.createTerminal({
        name: `Incan Check: ${path.basename(filePath)}`,
        cwd: path.dirname(filePath),
    });
    
    terminal.show();
    terminal.sendText(`${shellQuote(compiler.command)} ${shellQuote(filePath)}`);
}

export function activate(context: vscode.ExtensionContext) {
    outputChannel = vscode.window.createOutputChannel('Incan');
    
    // Register run/check commands
    context.subscriptions.push(
        vscode.commands.registerCommand('incan.runFile', runIncanFile),
        vscode.commands.registerCommand('incan.checkFile', checkIncanFile),
        vscode.commands.registerCommand('incan.doctor', showDoctor)
    );

    const config = vscode.workspace.getConfiguration('incan');
    const lspEnabled = config.get<boolean>('lsp.enabled', true);

    if (!lspEnabled) {
        outputChannel.appendLine('Incan LSP is disabled');
        return;
    }

    const server = getLspResolution();
    const compiler = getCompilerResolution();
    outputChannel.appendLine('Incan extension binary resolution');
    outputChannel.appendLine('================================');
    logResolution(server);
    logResolution(compiler);
    warnForResolution(server);

    // Server options - run the LSP binary
    const serverOptions: ServerOptions = {
        run: {
            command: server.command,
            transport: TransportKind.stdio,
        },
        debug: {
            command: server.command,
            transport: TransportKind.stdio,
        },
    };

    // Client options
    const clientOptions: LanguageClientOptions = {
        // Register for Incan files
        documentSelector: [
            { scheme: 'file', language: 'incan' },
            { scheme: 'untitled', language: 'incan' },
        ],
        synchronize: {
            // Watch .incn files for changes
            fileEvents: vscode.workspace.createFileSystemWatcher('**/*.incn'),
        },
    };

    // Create and start the client
    client = new LanguageClient(
        'incanLanguageServer',
        'Incan Language Server',
        serverOptions,
        clientOptions
    );

    // Start the client (also launches the server)
    client.start().then(() => {
        console.log('Incan Language Server started');
    }).catch((error) => {
        console.error('Failed to start Incan Language Server:', error);
        vscode.window.showWarningMessage(
            `Incan LSP failed to start. Make sure 'incan-lsp' is installed and in your PATH. ` +
            `You can also set the path in settings (incan.lsp.path).`
        );
    });

    // Register the client for disposal
    context.subscriptions.push({
        dispose: () => {
            if (client) {
                client.stop();
            }
        }
    });
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
