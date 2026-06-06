import * as assert from 'assert/strict';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import test from 'node:test';

import {
    BinaryResolution,
    commandCandidates,
    findWorkspaceBinary,
    formatDoctorReport,
    resolveBinary,
} from './binaryResolution';

function tempRoot(): string {
    return fs.mkdtempSync(path.join(os.tmpdir(), 'incan-vscode-resolution-'));
}

function writeExecutable(filePath: string) {
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, '#!/bin/sh\nexit 0\n');
    fs.chmodSync(filePath, 0o755);
}

function writePathCommand(dir: string, command: string): string {
    const candidate = commandCandidates(dir, command)[0];
    writeExecutable(candidate);
    return candidate;
}

test('findWorkspaceBinary prefers debug before release', () => {
    const root = tempRoot();
    const debugPath = path.join(root, 'target', 'debug', 'incan-lsp');
    const releasePath = path.join(root, 'target', 'release', 'incan-lsp');
    writeExecutable(releasePath);
    writeExecutable(debugPath);

    const resolution = findWorkspaceBinary('incan-lsp', [root]);

    assert.deepEqual(resolution, {
        path: debugPath,
        folder: root,
    });
});

test('resolveBinary uses configured paths before workspace and PATH', () => {
    const root = tempRoot();
    const workspaceBinary = path.join(root, 'target', 'debug', 'incan');
    const configuredBinary = path.join(root, 'configured', 'incan');
    writeExecutable(workspaceBinary);
    writeExecutable(configuredBinary);

    const resolution = resolveBinary({
        binaryName: 'incan',
        settingKey: 'incan.compiler.path',
        configuredPath: configuredBinary,
        workspaceFolders: [root],
        pathEnv: '',
    });

    assert.equal(resolution.command, configuredBinary);
    assert.equal(resolution.source, 'setting');
    assert.equal(resolution.exists, true);
    assert.equal(resolution.executable, true);
    assert.deepEqual(resolution.warnings, []);
});

test('resolveBinary uses workspace binaries before PATH', () => {
    const root = tempRoot();
    const pathDir = path.join(root, 'path-bin');
    const workspaceBinary = path.join(root, 'target', 'debug', 'incan-lsp');
    writeExecutable(workspaceBinary);
    writePathCommand(pathDir, 'incan-lsp');

    const resolution = resolveBinary({
        binaryName: 'incan-lsp',
        settingKey: 'incan.lsp.path',
        configuredPath: '',
        workspaceFolders: [root],
        pathEnv: pathDir,
    });

    assert.equal(resolution.command, workspaceBinary);
    assert.equal(resolution.source, 'workspace');
    assert.equal(resolution.workspaceFolder, root);
});

test('resolveBinary falls back to PATH when no setting or workspace binary exists', () => {
    const root = tempRoot();
    const pathDir = path.join(root, 'path-bin');
    const pathBinary = writePathCommand(pathDir, 'incan');

    const resolution = resolveBinary({
        binaryName: 'incan',
        settingKey: 'incan.compiler.path',
        configuredPath: '',
        workspaceFolders: [root],
        pathEnv: pathDir,
    });

    assert.equal(resolution.command, pathBinary);
    assert.equal(resolution.source, 'path');
    assert.equal(resolution.exists, true);
    assert.equal(resolution.executable, true);
});

test('resolveBinary reports shell syntax and missing configured paths', () => {
    const resolution = resolveBinary({
        binaryName: 'incan-lsp',
        settingKey: 'incan.lsp.path',
        configuredPath: '~/dev/incan/target/debug/incan-lsp',
        workspaceFolders: [],
        pathEnv: '',
    });

    assert.equal(resolution.source, 'setting');
    assert.equal(resolution.exists, false);
    assert.equal(resolution.executable, false);
    assert.match(resolution.warnings.join('\n'), /literal executable path/);
    assert.match(resolution.warnings.join('\n'), /path does not exist/);
});

test('formatDoctorReport includes resolved binaries and CLI counterpart', () => {
    const compiler: BinaryResolution = {
        name: 'incan',
        command: '/tmp/incan',
        source: 'path',
        exists: true,
        executable: true,
        warnings: [],
    };
    const lsp: BinaryResolution = {
        name: 'incan-lsp',
        command: '/tmp/incan-lsp',
        source: 'setting',
        settingKey: 'incan.lsp.path',
        exists: false,
        executable: false,
        warnings: ['incan-lsp path does not exist: /tmp/incan-lsp'],
    };

    const report = formatDoctorReport(compiler, lsp).join('\n');

    assert.match(report, /Incan toolchain doctor/);
    assert.match(report, /incan tools doctor --format json/);
    assert.match(report, /source: setting/);
    assert.match(report, /warning: incan-lsp path does not exist/);
});
