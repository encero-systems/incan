import * as fs from 'fs';
import * as path from 'path';

export type BinaryResolutionSource = 'setting' | 'workspace' | 'path';

export interface BinaryResolution {
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

export interface WorkspaceBinary {
    path: string;
    folder: string;
}

export interface ResolveBinaryOptions {
    binaryName: string;
    settingKey: string;
    configuredPath: string;
    workspaceFolders: string[];
    pathEnv?: string;
}

export function findWorkspaceBinary(binaryName: string, workspaceFolders: readonly string[]): WorkspaceBinary | undefined {
    const candidates = [
        path.join('target', 'debug', binaryName),
        path.join('target', 'release', binaryName),
    ];

    for (const folder of workspaceFolders) {
        for (const rel of candidates) {
            const abs = path.join(folder, rel);
            if (fs.existsSync(abs)) {
                return {
                    path: abs,
                    folder,
                };
            }
        }
    }
    return undefined;
}

export function pathHasShellSyntax(value: string): boolean {
    return value.startsWith('~') || value.includes('$') || value.includes('`');
}

export function isExecutableFile(value: string): boolean {
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

export function readSymlinkTarget(value: string): string | undefined {
    try {
        if (fs.lstatSync(value).isSymbolicLink()) {
            return fs.readlinkSync(value);
        }
    } catch {
        return undefined;
    }
    return undefined;
}

export function commandCandidates(dir: string, command: string): string[] {
    if (process.platform !== 'win32') {
        return [path.join(dir, command)];
    }
    const pathExt = process.env.PATHEXT ?? '.EXE;.CMD;.BAT;.COM';
    return pathExt.split(';').map(ext => path.join(dir, `${command}${ext}`));
}

export function findOnPath(command: string, pathEnv = process.env.PATH): string | undefined {
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

export function enrichPathStatus(resolution: BinaryResolution, pathEnv?: string): BinaryResolution {
    const command = resolution.command;
    const concretePath = hasPathSeparator(command) || path.isAbsolute(command)
        ? command
        : findOnPath(command, pathEnv);

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

export function resolveBinary(options: ResolveBinaryOptions): BinaryResolution {
    const configuredPath = options.configuredPath.trim();

    if (configuredPath) {
        const warnings = pathHasShellSyntax(configuredPath)
            ? [`${options.settingKey} is a literal executable path; shell syntax is not expanded: ${configuredPath}`]
            : [];
        return enrichPathStatus({
            name: options.binaryName,
            command: configuredPath,
            source: 'setting',
            settingKey: options.settingKey,
            warnings,
        }, options.pathEnv);
    }

    const workspaceBinary = findWorkspaceBinary(options.binaryName, options.workspaceFolders);
    if (workspaceBinary) {
        return enrichPathStatus({
            name: options.binaryName,
            command: workspaceBinary.path,
            source: 'workspace',
            workspaceFolder: workspaceBinary.folder,
            warnings: [],
        }, options.pathEnv);
    }

    return enrichPathStatus({
        name: options.binaryName,
        command: options.binaryName,
        source: 'path',
        warnings: [],
    }, options.pathEnv);
}

export function cargoBinReport(binaryName: string, home = process.env.HOME ?? process.env.USERPROFILE): string[] {
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

export function formatResolution(resolution: BinaryResolution): string[] {
    const lines = [
        `${resolution.name}: ${resolution.command}`,
        `  source: ${resolution.source}`,
    ];
    if (resolution.settingKey) {
        lines.push(`  setting: ${resolution.settingKey}`);
    }
    if (resolution.workspaceFolder) {
        lines.push(`  workspace: ${resolution.workspaceFolder}`);
    }
    lines.push(`  exists: ${resolution.exists ?? 'unknown'}`);
    lines.push(`  executable: ${resolution.executable ?? 'unknown'}`);
    if (resolution.symlinkTarget) {
        lines.push(`  symlink target: ${resolution.symlinkTarget}`);
    }
    for (const warning of resolution.warnings) {
        lines.push(`  warning: ${warning}`);
    }
    return lines;
}

export function formatDoctorReport(compiler: BinaryResolution, lsp: BinaryResolution): string[] {
    return [
        'Incan toolchain doctor',
        '======================',
        '',
        'Resolved binaries:',
        ...formatResolution(compiler),
        ...formatResolution(lsp),
        '',
        'Cargo bin links:',
        ...cargoBinReport('incan'),
        ...cargoBinReport('incan-lsp'),
        '',
        'CLI counterpart:',
        '  incan tools doctor',
        '  incan tools doctor --format json',
        '',
        'Recovery:',
        '  - Run `make build` from the checkout you want to use.',
        '  - Leave incan.lsp.path and incan.compiler.path empty unless you need fixed binaries.',
        '  - Use literal executable paths; $HOME and ~ are not expanded in settings.',
        '  - Reload VS Code/Cursor after rebuilding or changing paths.',
    ];
}
