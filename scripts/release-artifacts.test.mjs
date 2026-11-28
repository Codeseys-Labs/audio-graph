import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("../", import.meta.url));
const script = join(repo, "scripts", "release-artifacts.mjs");
const macIcon = "icons/icon.icns";
const pngIcon = "icons/32x32.png";

function configure(cwd) {
    return spawnSync(process.execPath, [script, "configure"], { cwd, encoding: "utf8" });
}

function iconChunk(type, payload) {
    const header = Buffer.alloc(8);
    header.write(type, "ascii");
    header.writeUInt32BE(header.length + payload.length, 4);
    return Buffer.concat([header, payload]);
}

const iconEntry = iconChunk("ic07", Buffer.from([1]));

function fixture(t, {
    bundle = { icon: [pngIcon, macIcon] },
    png = "png fixture",
    icns = iconChunk("icns", iconEntry),
} = {}) {
    const cwd = mkdtempSync(join(tmpdir(), "audio-graph-release-"));
    t.after(() => rmSync(cwd, { recursive: true, force: true }));
    const tauri = join(cwd, "src-tauri");
    mkdirSync(join(tauri, "icons"), { recursive: true });
    writeFileSync(join(tauri, "tauri.conf.json"), JSON.stringify({ bundle }));
    if (png !== null) {
        writeFileSync(join(tauri, pngIcon), png);
    }
    if (icns !== null) {
        writeFileSync(join(tauri, macIcon), icns);
    }
    return cwd;
}

test("configure validates the repository icons and emits a bundling overlay", () => {
    const result = configure(repo);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), { bundle: { active: true } });
});

const invalidConfigurations = [
    ["omitted bundle.icon", { bundle: {} }, /bundle\.icon/],
    ["empty bundle.icon", { bundle: { icon: [] } }, /bundle\.icon/],
    ["invalid icon path", { bundle: { icon: [macIcon, null] } }, /bundle\.icon/],
    ["undeclared ICNS", { bundle: { icon: [pngIcon] } }, /icons\/icon\.icns/],
    ["missing ICNS", { icns: null }, /icon\.icns/],
    ["missing icon", { png: null }, /32x32\.png/],
    ["empty icon", { png: "" }, /32x32\.png/],
    ["wrong ICNS signature", { icns: iconChunk("test", iconEntry) }, /ICNS/],
    ["OSType inside an unrelated chunk", { icns: iconChunk("icns", iconChunk("TOC ", Buffer.from("ic07"))) }, /ICNS/],
    ["empty icon chunk", { icns: iconChunk("icns", iconChunk("ic07", Buffer.alloc(0))) }, /ICNS/],
    ["truncated icon chunk", { icns: iconChunk("icns", iconEntry.subarray(0, 8)) }, /ICNS/],
    ["incorrect ICNS length", { icns: iconChunk("icns", iconEntry).subarray(0, 16) }, /ICNS/],
    ["trailing bytes after a valid chunk", { icns: iconChunk("icns", Buffer.concat([iconEntry, Buffer.from([0])])) }, /ICNS/],
];

for (const [name, files, error] of invalidConfigurations) {
    test(`configure rejects ${name}`, (t) => {
        const result = configure(fixture(t, files));
        assert.equal(result.status, 1, result.stderr);
        assert.equal(result.stdout, "");
        assert.match(result.stderr, error);
    });
}
