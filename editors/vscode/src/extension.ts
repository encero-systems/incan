/**
 * Incan Language Extension for VS Code / Cursor
 * 
 * Provides:
 * - Syntax highlighting (via TextMate grammar)
 * - LSP integration for real-time diagnostics, hover, and go-to-definition
 * - Run/Check commands for Incan files
 */

import * as path from 'path';
import * as fs from 'fs';
import * as vscode from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    TransportKind,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel;

type BinaryResolutionSource = 'setting' | 'workspace' | 'path';

interface BinaryResolution {
    name: string;
    command: string;
    source: BinaryResolutionSource;
    settingKey?: string;
    workspaceFolder?: string;
    exists?: boolean;
    executable?: boolean;
    symlinkTarget?: string;
    warnings: string[];
}

interface WorkspaceBinary {
    path: string;
    folder: string;
}

function findWorkspaceBinary(binaryName: string): WorkspaceBinary | undefined {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || folders.length === 0) {
        return undefined;
    }

    // Prefer debug while developing; fall back to release.
    const candidates = [
        path.join('target', 'debug', binaryName),
        path.join('target', 'release', binaryName),
    ];

    for (const folder of folders) {
        for (const rel of candidates) {
            const abs = path.join(folder.uri.fsPath, rel);
            if (fs.existsSync(abs)) {
                return {
                    path: abs,
                    folder: folder.uri.fsPath,
                };
            }
        }
    }
    return undefined;
}

function pathHasShellSyntax(value: string): boolean {
    return value.startsWith('~') || value.includes('$') || value.includes('`');
}

function isExecutableFile(value: string): boolean {
    try {
        const stat = fs.statSync(value);
        if (!stat.isFile()) {
            return false;
        }
        if (process.platform === 'win32') {
            return true;
        }
        fs.accessSync(value, fs.constants.X_OK);
        return true;
    } catch {
        return false;
    }
}

function readSymlinkTarget(value: string): string | undefined {
    try {
        if (fs.lstatSync(value).isSymbolicLink()) {
            return fs.readlinkSync(value);
        }
    } catch {
        return undefined;
    }
    return undefined;
}

function commandCandidates(dir: string, command: string): string[] {
    if (process.platform !== 'win32') {
        return [path.join(dir, command)];
    }
    const pathExt = process.env.PATHEXT ?? '.EXE;.CMD;.BAT;.COM';
    return pathExt.split(';').map(ext => path.join(dir, `${command}${ext}`));
}

function findOnPath(command: string): string | undefined {
    const pathEnv = process.env.PATH;
    if (!pathEnv) {
        return undefined;
    }
    for (const dir of pathEnv.split(path.delimiter)) {
        for (const candidate of commandCandidates(dir, command)) {
            if (isExecutableFile(candidate)) {
                return candidate;
            }
        }
    }
    return undefined;
}

function hasPathSeparator(value: string): boolean {
    return value.includes('/') || value.includes('\\');
}

function enrichPathStatus(resolution: BinaryResolution): BinaryResolution {
    const command = resolution.command;
    const concretePath = hasPathSeparator(command) || path.isAbsolute(command)
        ? command
        : findOnPath(command);

    if (!concretePath) {
        return {
            ...resolution,
            exists: false,
            executable: false,
            warnings: [
                ...resolution.warnings,
                `${resolution.name} was not found on PATH.`,
            ],
        };
    }

    const exists = fs.existsSync(concretePath);
    const executable = isExecutableFile(concretePath);
    const warnings = [...resolution.warnings];
    if (!exists) {
        warnings.push(`${resolution.name} path does not exist: ${concretePath}`);
    } else if (!executable) {
        warnings.push(`${resolution.name} path is not executable: ${concretePath}`);
    }

    return {
        ...resolution,
        command: concretePath,
        exists,
        executable,
        symlinkTarget: readSymlinkTarget(concretePath),
        warnings,
    };
}

function resolveBinary(binaryName: string, settingName: 'compiler.path' | 'lsp.path'): BinaryResolution {
    const config = vscode.workspace.getConfiguration('incan');
    const configuredPath = config.get<string>(settingName, '').trim();
    const settingKey = `incan.${settingName}`;

    if (configuredPath) {
        const warnings = pathHasShellSyntax(configuredPath)
            ? [`${settingKey} is a literal executable path; shell syntax is not expanded: ${configuredPath}`]
            : [];
        return enrichPathStatus({
            name: binaryName,
            command: configuredPath,
            source: 'setting',
            settingKey,
            warnings,
        });
    }

    const workspaceBinary = findWorkspaceBinary(binaryName);
    if (workspaceBinary) {
        return enrichPathStatus({
            name: binaryName,
            command: workspaceBinary.path,
            source: 'workspace',
            workspaceFolder: workspaceBinary.folder,
            warnings: [],
        });
    }

    return enrichPathStatus({
        name: binaryName,
        command: binaryName,
        source: 'path',
        warnings: [],
    });
}

function getCompilerResolution(): BinaryResolution {
    return resolveBinary('incan', 'compiler.path');
}

function getLspResolution(): BinaryResolution {
    return resolveBinary('incan-lsp', 'lsp.path');
}

function shellQuote(value: string): string {
    return `"${value.replace(/(["\\$`])/g, '\\$1')}"`;
}

function logResolution(resolution: BinaryResolution) {
    outputChannel.appendLine(`${resolution.name}: ${resolution.command}`);
    outputChannel.appendLine(`  source: ${resolution.source}`);
    if (resolution.settingKey) {
        outputChannel.appendLine(`  setting: ${resolution.settingKey}`);
    }
    if (resolution.workspaceFolder) {
        outputChannel.appendLine(`  workspace: ${resolution.workspaceFolder}`);
    }
    outputChannel.appendLine(`  exists: ${resolution.exists ?? 'unknown'}`);
    outputChannel.appendLine(`  executable: ${resolution.executable ?? 'unknown'}`);
    if (resolution.symlinkTarget) {
        outputChannel.appendLine(`  symlink target: ${resolution.symlinkTarget}`);
    }
    for (const warning of resolution.warnings) {
        outputChannel.appendLine(`  warning: ${warning}`);
    }
}

function warnForResolution(resolution: BinaryResolution) {
    if (resolution.warnings.length === 0) {
        return;
    }
    vscode.window.showWarningMessage(`Incan ${resolution.name} setup issue: ${resolution.warnings[0]}`);
}

function cargoBinReport(binaryName: string): string[] {
    const home = process.env.HOME ?? process.env.USERPROFILE;
    if (!home) {
        return [`~/.cargo/bin/${binaryName}: home directory unavailable`];
    }
    const cargoBinPath = path.join(home, '.cargo', 'bin', binaryName);
    const exists = fs.existsSync(cargoBinPath);
    const executable = exists && isExecutableFile(cargoBinPath);
    const symlinkTarget = exists ? readSymlinkTarget(cargoBinPath) : undefined;
    return [
        `${cargoBinPath}`,
        `  exists: ${exists}`,
        `  executable: ${executable}`,
        `  symlink target: ${symlinkTarget ?? '(not a symlink or unavailable)'}`,
    ];
}

function writeDoctorReport() {
    const compiler = getCompilerResolution();
    const lsp = getLspResolution();
    outputChannel.appendLine('Incan toolchain doctor');
    outputChannel.appendLine('======================');
    outputChannel.appendLine('');
    outputChannel.appendLine('Resolved binaries:');
    logResolution(compiler);
    logResolution(lsp);
    outputChannel.appendLine('');
    outputChannel.appendLine('Cargo bin links:');
    for (const line of cargoBinReport('incan')) {
        outputChannel.appendLine(line);
    }
    for (const line of cargoBinReport('incan-lsp')) {
        outputChannel.appendLine(line);
    }
    outputChannel.appendLine('');
    outputChannel.appendLine('CLI counterpart:');
    outputChannel.appendLine('  incan tools doctor');
    outputChannel.appendLine('  incan tools doctor --format json');
    outputChannel.appendLine('');
    outputChannel.appendLine('Recovery:');
    outputChannel.appendLine('  - Run `make build` from the checkout you want to use.');
    outputChannel.appendLine('  - Leave incan.lsp.path and incan.compiler.path empty unless you need fixed binaries.');
    outputChannel.appendLine('  - Use literal executable paths; $HOME and ~ are not expanded in settings.');
    outputChannel.appendLine('  - Reload VS Code/Cursor after rebuilding or changing paths.');
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










