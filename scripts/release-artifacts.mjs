import { createHash } from "node:crypto";
import {
    copyFileSync,
    createReadStream,
    existsSync,
    lstatSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";

// Match the active targets in src-tauri/tauri.conf.json. Read only the
// deliverables directly under their bundle directories, leaving staging trees out.
const platforms = {
    linux: {
        output: "release",
        binary: "audio-graph",
        bundles: { appimage: ".AppImage", deb: ".deb" },
    },
    macos: {
        output: "universal-apple-darwin/release",
        binary: "audio-graph",
        bundles: { dmg: ".dmg", macos: ".app.tar.gz" },
    },
    windows: {
        output: "release",
        binary: "audio-graph.exe",
        bundles: { nsis: "-setup.exe" },
    },
};

const metadataName = (platform) => "rsac-" + platform + ".txt";
const inventoryName = (platform) => "artifacts-" + platform + ".sha256";
const downloadPrefix = "release-build-";

function deliverables(root, platform) {
    if (!Object.hasOwn(platforms, platform)) {
        throw new Error("Unknown release platform: " + platform);
    }
    const { binary, bundles } = platforms[platform];
    const files = [binary];
    for (const [directory, suffix] of Object.entries(bundles)) {
        const bundleDir = "bundle/" + directory;
        const absoluteDir = join(root, bundleDir);
        const names = readdirSync(absoluteDir).filter((name) => name.endsWith(suffix));
        if (names.length === 0) {
            throw new Error("Missing " + suffix + " deliverable in " + absoluteDir);
        }
        for (const name of names) {
            const relativePath = bundleDir + "/" + name;
            files.push(relativePath);
            if (existsSync(join(root, relativePath + ".sig"))) {
                files.push(relativePath + ".sig");
            }
        }
    }
    return files;
}

function requiredSize(path) {
    const stat = lstatSync(path);
    if (!stat.isFile() || stat.size === 0) {
        throw new Error("Expected a non-empty release file: " + path);
    }
    return stat.size;
}

async function inventory(root, files) {
    const rows = [];
    for (const relativePath of files.toSorted()) {
        const path = join(root, relativePath);
        const size = requiredSize(path);
        const hash = createHash("sha256");
        for await (const chunk of createReadStream(path)) {
            hash.update(chunk);
        }
        rows.push([hash.digest("hex"), size, relativePath].join("\t"));
    }
    return rows.join("\n") + "\n";
}

async function stage(platform) {
    if (!Object.hasOwn(platforms, platform)) {
        throw new Error("Unknown release platform: " + platform);
    }
    const source = join("src-tauri", "target", platforms[platform].output);
    const destination = join(process.env.RUNNER_TEMP, "release-metadata");
    const metadata = metadataName(platform);
    requiredSize(join(destination, metadata));
    const files = deliverables(source, platform);
    for (const file of files) {
        const from = join(source, file);
        const to = join(destination, file);
        requiredSize(from);
        mkdirSync(dirname(to), { recursive: true });
        copyFileSync(from, to);
    }
    const manifest = await inventory(destination, [...files, metadata]);
    writeFileSync(join(destination, inventoryName(platform)), manifest);
    process.stdout.write(manifest);
}

async function verifyDownloads() {
    const destination = join(process.env.RUNNER_TEMP, "release-download");
    const names = readdirSync(destination);
    const groups = names.filter((name) => name.startsWith(downloadPrefix)).sort()
        .map((name) => [name.slice(downloadPrefix.length), join(destination, name)]);
    // download-artifact v8 omits the artifact-name directory for one match.
    if (groups.length === 0) {
        const matches = Object.keys(platforms).filter((platform) => names.includes(inventoryName(platform)));
        if (matches.length !== 1) {
            throw new Error("Expected at least one complete platform artifact.");
        }
        groups.push([matches[0], destination]);
    }

    const assets = [];
    for (const [platform, root] of groups) {
        const files = [...deliverables(root, platform), metadataName(platform)];
        const manifest = inventoryName(platform);
        if (readFileSync(join(root, manifest), "utf8") !== await inventory(root, files)) {
            throw new Error("Release artifact inventory mismatch for " + platform);
        }
        // The Mac app archive is the standalone release asset. The raw Mach-O
        // remains available in the Actions artifact.
        const publishFiles = platform === "macos" ? files.filter((file) => file !== platforms.macos.binary) : files;
        assets.push(...[...publishFiles, manifest].map((file) => join(root, file)));
    }
    process.stdout.write(assets.join("\0") + "\0");
}

const [mode, platform] = process.argv.slice(2);
if (mode === "stage") {
    await stage(platform);
} else if (mode === "verify") {
    await verifyDownloads();
} else {
    throw new Error("Expected stage <platform> or verify.");
}
