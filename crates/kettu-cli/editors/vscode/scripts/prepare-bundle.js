const fs = require("fs");
const path = require("path");

const TARGET_PLATFORM_BY_HOST = {
    "darwin-arm64": "darwin-arm64",
    "darwin-x64": "darwin-x64",
    "linux-arm64": "linux-arm64",
    "linux-x64": "linux-x64",
    "win32-arm64": "win32-arm64",
    "win32-x64": "win32-x64",
};

function resolveTargetPlatform() {
    if (process.env.KETTU_TARGET_PLATFORM) {
        return process.env.KETTU_TARGET_PLATFORM;
    }

    const hostKey = `${process.platform}-${process.arch}`;
    const targetPlatform = TARGET_PLATFORM_BY_HOST[hostKey];
    if (!targetPlatform) {
        throw new Error(`Unsupported host platform ${hostKey}`);
    }

    return targetPlatform;
}

function executableName(targetPlatform) {
    return targetPlatform.startsWith("win32-") ? "kettu.exe" : "kettu";
}

function resolveServerPath(targetPlatform) {
    if (process.env.KETTU_SERVER_PATH) {
        return process.env.KETTU_SERVER_PATH;
    }

    const workspaceRoot = path.resolve(__dirname, "..", "..", "..", "..", "..");
    return path.join(workspaceRoot, "target", "release", executableName(targetPlatform));
}

function main() {
    const targetPlatform = resolveTargetPlatform();
    const serverPath = resolveServerPath(targetPlatform);

    if (!fs.existsSync(serverPath)) {
        throw new Error(`Compiler binary not found at ${serverPath}`);
    }

    const bundleDir = path.join(__dirname, "..", "bin", targetPlatform);
    const destination = path.join(bundleDir, executableName(targetPlatform));

    fs.rmSync(bundleDir, { recursive: true, force: true });
    fs.mkdirSync(bundleDir, { recursive: true });
    fs.copyFileSync(serverPath, destination);

    if (!targetPlatform.startsWith("win32-")) {
        fs.chmodSync(destination, 0o755);
    }

    console.log(`Bundled ${serverPath} -> ${destination}`);
}

main();
