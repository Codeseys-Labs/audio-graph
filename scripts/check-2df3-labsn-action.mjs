import { readFileSync } from "node:fs";
import process from "node:process";

const workflowPath = new URL(
    "../.github/workflows/2df3-native-durability.yml",
    import.meta.url,
);
const workflow = readFileSync(workflowPath, "utf8");
const canonicalDurability = readFileSync(
    new URL("../src-tauri/src/persistence/canonical_durability.rs", import.meta.url),
    "utf8",
);
const sessionArtifactManifest = readFileSync(
    new URL("../src-tauri/src/persistence/session_artifact_manifest.rs", import.meta.url),
    "utf8",
);
const cargoManifest = readFileSync(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8");
const cargoLock = readFileSync(new URL("../src-tauri/Cargo.lock", import.meta.url), "utf8");

const ACTION_PIN =
    "LABSN/sound-ci-helpers@d08c889a7bba7d9b1b059f8f76dac4672ea3a9cf";
const LICENSE_CONDITION =
    "matrix.os == 'windows' && inputs.confirm_vb_cable_professional_license == true";

function invariant(condition, message) {
    if (!condition) {
        throw new Error(message);
    }
}

function countArtifactLines(artifact, expected) {
    return artifact.split("\n").filter((line) => line === expected).length;
}

const CC9A_MACOS_DIAGNOSTIC_MARKER = "CC9A_MACOS_DIAGNOSTIC ";
const CC9A_INVENTORY_SED_PREFIX =
    "s/^CC9A_MACOS_DIAGNOSTIC root .* inventory_count=\\([0-9][0-9]*\\) root_fsid_result=";
const POSIX_CC9A_INVENTORY_AVAILABLE_SED = `${CC9A_INVENTORY_SED_PREFIX}available$/\\1/p`;
const POSIX_CC9A_INVENTORY_UNAVAILABLE_SED = `${CC9A_INVENTORY_SED_PREFIX}unavailable$/\\1/p`;
const GNU_CC9A_INVENTORY_ALTERNATION_SED =
    `${CC9A_INVENTORY_SED_PREFIX}\\(available\\|unavailable\\)$/\\1/p`;
const CC9A_LIVE_INLINE_LOG = [
    "running 3 tests",
    "test persistence::canonical_durability::tests::cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation ... CC9A_MACOS_DIAGNOSTIC root canonical_root=/private/var/folders/pm/cmklcsfj60nd7nfc79g8xmbc0000gn/T/ag-canonical-production-binding-first-12495-0 root_dev=16777229 inventory_count=2 root_fsid_result=available",
    "CC9A_MACOS_DIAGNOSTIC observation index=0 mount_path=/ filesystem_class=Apfs filesystem_string=apfs metadata_result=available dev=16777229 same_root_dev=true fsid_result=available same_root_fsid=false read_only=true removable=false",
    "CC9A_MACOS_DIAGNOSTIC observation index=1 mount_path=/System/Volumes/Data filesystem_class=Apfs filesystem_string=apfs metadata_result=available dev=16777229 same_root_dev=true fsid_result=available same_root_fsid=true read_only=false removable=false",
    "CC9A_MACOS_DIAGNOSTIC summary metadata_unavailable_count=0 same_root_dev_count=2 branch=ambiguous",
    "CC9A_MACOS_DIAGNOSTIC exact root_equals_data=true root_differs_system=true same_root_fsid_count=1 probe_unavailable_count=0 root_before_after_stable=true selection_authority=fsid mounted_on_text_authority=false",
    "ok",
    "test persistence::session_artifact_manifest::tests::cc9a_native_qualified_initial_cas_has_parent_barrier ... ok",
    "test persistence::session_artifact_manifest::tests::cc9a_native_qualified_replacement_refuses_foreign_open_head_before_temp_creation ... ok",
    "",
    "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 1672 filtered out; finished in 0.11s",
].join("\n");

function priorCc9aMacosDiagnostics(log) {
    return log.split("\n").filter((line) => line.startsWith(CC9A_MACOS_DIAGNOSTIC_MARKER));
}

function correctedCc9aMacosDiagnostics(log) {
    return log.split("\n").flatMap((line) => {
        const marker = line.indexOf(CC9A_MACOS_DIAGNOSTIC_MARKER);
        const inlineTestDiagnostic =
            /^test [^ ]*cc9a_native_[^ ]* \.\.\. CC9A_MACOS_DIAGNOSTIC /.test(line);
        if (marker === 0 || inlineTestDiagnostic) {
            return [line.slice(marker)];
        }
        return [];
    });
}

function cc9aMacosDiagnosticCounts(artifact) {
    const lines = artifact.split("\n").filter(Boolean);
    const rootPattern =
        /^CC9A_MACOS_DIAGNOSTIC root canonical_root=.* root_dev=.* inventory_count=([0-9]+) root_fsid_result=(available|unavailable)$/;
    const observationPattern = /^CC9A_MACOS_DIAGNOSTIC observation /;
    const observationSchemaPattern =
        /^CC9A_MACOS_DIAGNOSTIC observation index=[0-9]+ mount_path=.* filesystem_class=[^ ]+ filesystem_string=.* metadata_result=(available|unavailable) dev=([0-9]+|unavailable) same_root_dev=(true|false|unavailable) fsid_result=(available|unavailable) same_root_fsid=(true|false|unavailable) read_only=(true|false) removable=(true|false)$/;
    const summaryPattern =
        /^CC9A_MACOS_DIAGNOSTIC summary metadata_unavailable_count=[0-9]+ same_root_dev_count=[0-9]+ branch=(root_missing|zero_match_clean|zero_match_with_unavailable|unique|ambiguous|unique_then_validate_mismatch)$/;
    const exactLine =
        "CC9A_MACOS_DIAGNOSTIC exact root_equals_data=true root_differs_system=true same_root_fsid_count=1 probe_unavailable_count=0 root_before_after_stable=true selection_authority=fsid mounted_on_text_authority=false";
    const inventory = lines.flatMap((line) => {
        const match = line.match(rootPattern);
        return match ? [Number(match[1])] : [];
    });
    const rootFieldLabels = ["canonical_root=", "root_dev=", "inventory_count=", "root_fsid_result="];
    return {
        total: lines.filter((line) => line.startsWith(CC9A_MACOS_DIAGNOSTIC_MARKER)).length,
        inventory: inventory.length === 1 ? inventory[0] : Number.NaN,
        observations: lines.filter((line) => observationPattern.test(line)).length,
        observationSchemas: lines.filter((line) => observationSchemaPattern.test(line)).length,
        roots: lines.filter((line) => rootPattern.test(line)).length,
        rootFieldUniqueness: lines.filter(
            (line) =>
                rootPattern.test(line) &&
                rootFieldLabels.every((label) => line.split(label).length - 1 === 1),
        ).length,
        summaries: lines.filter((line) => summaryPattern.test(line)).length,
        exact: lines.filter((line) => line === exactLine).length,
    };
}

function priorCc9aMacosDiagnosticSummaryAccepts(artifact) {
    const counts = cc9aMacosDiagnosticCounts(artifact);
    return (
        counts.roots === 1 &&
        counts.summaries === 1 &&
        Number.isInteger(counts.inventory) &&
        counts.observations === counts.inventory &&
        counts.observationSchemas === counts.inventory &&
        counts.exact === 1
    );
}

function correctedCc9aMacosDiagnosticSummaryAccepts(artifact) {
    const counts = cc9aMacosDiagnosticCounts(artifact);
    return (
        counts.total === 5 &&
        counts.inventory === 2 &&
        counts.observations === 2 &&
        counts.observationSchemas === 2 &&
        counts.roots === 1 &&
        counts.summaries === 1 &&
        counts.exact === 1
    );
}

function fieldUniqueCc9aMacosDiagnosticSummaryAccepts(artifact) {
    const counts = cc9aMacosDiagnosticCounts(artifact);
    return correctedCc9aMacosDiagnosticSummaryAccepts(artifact) && counts.rootFieldUniqueness === 1;
}

function posixCc9aInventoryValues(artifact) {
    const patterns = [
        /^CC9A_MACOS_DIAGNOSTIC root .* inventory_count=([0-9]+) root_fsid_result=available$/,
        /^CC9A_MACOS_DIAGNOSTIC root .* inventory_count=([0-9]+) root_fsid_result=unavailable$/,
    ];
    return artifact.split("\n").flatMap((line) => {
        for (const pattern of patterns) {
            const match = line.match(pattern);
            if (match) {
                return [match[1]];
            }
        }
        return [];
    });
}

function posixCc9aInventorySummaryAccepts(artifact) {
    return posixCc9aInventoryValues(artifact).join("\n") === "2";
}

function priorCc9aNativeNames(log) {
    return log
        .split("\n")
        .flatMap((line) => {
            const fields = line.trim().split(/\s+/);
            if (
                fields[0] === "test" &&
                fields[1]?.includes("cc9a_native_") &&
                fields.at(-1) === "ok"
            ) {
                return [fields[1].replace(/:$/, "").replace(/^.*::/, "")];
            }
            return [];
        })
        .sort();
}

function correctedCc9aNativeNames(log) {
    const names = [];
    let pendingName = "";
    for (const line of log.split("\n")) {
        const testStart = line.match(/^test ([^ ]*::(cc9a_native_[^ ]+)) \.\.\. (.*)$/);
        if (testStart) {
            pendingName = "";
            if (testStart[3] === "ok") {
                names.push(testStart[2]);
            } else if (testStart[3].startsWith(CC9A_MACOS_DIAGNOSTIC_MARKER)) {
                pendingName = testStart[2];
            }
            continue;
        }
        if (pendingName && line.startsWith(CC9A_MACOS_DIAGNOSTIC_MARKER)) {
            continue;
        }
        if (pendingName && line === "ok") {
            names.push(pendingName);
            pendingName = "";
            continue;
        }
        pendingName = "";
    }
    return [...new Set(names)].sort();
}

function priorMacosMountSummaryAccepts(artifact) {
    return (
        countArtifactLines(artifact, "stat_target=probe") === 1 &&
        countArtifactLines(artifact, "stat_target=/") === 1 &&
        countArtifactLines(artifact, "stat_target=/System/Volumes/Data") === 1 &&
        countArtifactLines(artifact, "diskutil_target=probe") === 1 &&
        countArtifactLines(artifact, "diskutil_target=/") === 1 &&
        countArtifactLines(artifact, "diskutil_target=/System/Volumes/Data") === 1 &&
        countArtifactLines(artifact, "target=probe") === 1 &&
        countArtifactLines(artifact, "target=/") === 1 &&
        countArtifactLines(artifact, "target=/System/Volumes/Data") === 1 &&
        countArtifactLines(artifact, "resolved_mount=/System/Volumes/Data") === 2 &&
        countArtifactLines(artifact, "resolved_mount=/") === 1 &&
        countArtifactLines(artifact, "diskutil_exit=0") === 3 &&
        countArtifactLines(artifact, "target_count=3") === 1 &&
        countArtifactLines(artifact, "resolved_count=3") === 1 &&
        countArtifactLines(artifact, "success_count=3") === 1 &&
        countArtifactLines(artifact, "failure_count=0") === 1 &&
        countArtifactLines(artifact, "mount_record=present") === 1 &&
        countArtifactLines(artifact, "diagnostics_complete=true") === 1
    );
}

function correctedMacosMountSummaryAccepts(artifact) {
    return priorMacosMountSummaryAccepts(artifact) && countArtifactLines(artifact, "stat_exit=0") === 3;
}

function macosMountSummarySimulation(statExits) {
    return [
        "stat_target=probe",
        `stat_exit=${statExits[0]}`,
        "stat_target=/",
        `stat_exit=${statExits[1]}`,
        "stat_target=/System/Volumes/Data",
        `stat_exit=${statExits[2]}`,
        "target=probe",
        "target=/",
        "target=/System/Volumes/Data",
        "diskutil_target=probe",
        "diskutil_target=/",
        "diskutil_target=/System/Volumes/Data",
        "resolved_mount=/System/Volumes/Data",
        "resolved_mount=/",
        "resolved_mount=/System/Volumes/Data",
        "diskutil_exit=0",
        "diskutil_exit=0",
        "diskutil_exit=0",
        "target_count=3",
        "resolved_count=3",
        "success_count=3",
        "failure_count=0",
        "mount_record=present",
        "diagnostics_complete=true",
    ].join("\n");
}

function stepBody(source, name) {
    const marker = `      - name: ${name}\n`;
    const start = source.indexOf(marker);
    invariant(start >= 0, `missing workflow step: ${name}`);
    const next = source.indexOf("\n      - name: ", start + marker.length);
    return source.slice(start, next < 0 ? source.length : next);
}

function matrixRow(source, os) {
    const marker = `          - os: ${os}\n`;
    const start = source.indexOf(marker);
    invariant(start >= 0, `missing matrix row: ${os}`);
    const bodyStart = start + marker.length;
    const nextRow = source.indexOf("          - os: ", bodyStart);
    const env = source.indexOf("    env:\n", bodyStart);
    const end = nextRow >= 0 ? nextRow : env;
    invariant(end > bodyStart, `unterminated matrix row: ${os}`);
    return source.slice(start, end);
}

function mutateStep(source, name, search, replacement) {
    const body = stepBody(source, name);
    const mutated = body.replace(search, replacement);
    invariant(mutated !== body, `mutation target missing in workflow step: ${name}`);
    return source.replace(body, mutated);
}

function rustTest(source, name) {
    const marker = `fn ${name}`;
    const functionStart = source.indexOf(marker);
    invariant(functionStart >= 0, `missing Rust test: ${name}`);
    invariant(
        source.indexOf(marker, functionStart + marker.length) < 0,
        `duplicate Rust test: ${name}`,
    );
    const nextTest = source.indexOf("\n    #[", functionStart + marker.length);
    const prefixStart = Math.max(0, functionStart - 180);
    return source.slice(prefixStart, nextTest < 0 ? source.length : nextTest);
}

function mutateRustTest(source, name, search, replacement) {
    const test = rustTest(source, name);
    const mutated = test.replace(search, replacement);
    invariant(mutated !== test, `mutation target missing in Rust test: ${name}`);
    return source.replace(test, mutated);
}

function rustFunction(source, name) {
    const marker = `fn ${name}`;
    const functionStart = source.indexOf(marker);
    invariant(functionStart >= 0, `missing Rust function: ${name}`);
    invariant(
        source.indexOf(marker, functionStart + marker.length) < 0,
        `duplicate Rust function: ${name}`,
    );
    const nextItem = source.indexOf("\n#[", functionStart + marker.length);
    return source.slice(functionStart, nextItem < 0 ? source.length : nextItem);
}

function validate(source, canonicalSource = canonicalDurability, manifestSource = sessionArtifactManifest) {
    const prestateName = "Record LABSN Windows prestate";
    const actionName = "Install Windows virtual audio baseline with pinned LABSN action";
    const cleanupName = "Restore LABSN TrustedPublisher state";
    const canaryName = "Record bounded allowlisted Windows endpoint inventory";
    const durabilityName = "Run canonical durability filter (Windows)";
    const cc9aUnixName = "Run cc9a native qualification filter (Unix)";
    const cc9aWindowsName = "Run cc9a native qualification filter (Windows)";
    const macosDiagnosticsName = "Record macOS mount diagnostics";

    const prestate = stepBody(source, prestateName);
    const action = stepBody(source, actionName);
    const cleanup = stepBody(source, cleanupName);
    const canary = stepBody(source, canaryName);
    const durability = stepBody(source, durabilityName);
    const cc9aUnix = stepBody(source, cc9aUnixName);
    const cc9aWindows = stepBody(source, cc9aWindowsName);
    const macosDiagnostics = stepBody(source, macosDiagnosticsName);
    const windowsMatrix = matrixRow(source, "windows");
    const unixSummary = stepBody(source, "Summarize and enforce native exits (Unix)");
    const windowsSummary = stepBody(source, "Summarize and enforce native exits (Windows)");

    const positions = [prestateName, actionName, cleanupName, canaryName, durabilityName].map(
        (name) => source.indexOf(`      - name: ${name}\n`),
    );
    invariant(
        positions.every((position, index) => index === 0 || position > positions[index - 1]),
        "LABSN prestate, action, cleanup, endpoint canary, and Windows tests must stay ordered",
    );

    invariant(prestate.includes(`if: \${{ ${LICENSE_CONDITION} }}`), "prestate license gate drift");
    invariant(prestate.includes("id: labsn_prestate"), "prestate step must expose outputs");
    invariant(
        prestate.includes("publisher_match_count=") &&
            prestate.includes("preexisting_cable_endpoint_count="),
        "prestate must publish certificate and endpoint counts",
    );
    invariant(
        prestate.includes("if ($cableEndpoints.Count -ne 0)") &&
            prestate.includes("refusing a non-causal installation canary"),
        "prestate must refuse a pre-existing VB-CABLE endpoint",
    );
    invariant(
        prestate.includes("Get-PnpDevice -PresentOnly -ErrorAction Stop") &&
            prestate.includes("Where-Object Class -eq 'AudioEndpoint'") &&
            !prestate.includes("Get-PnpDevice -Class AudioEndpoint"),
        "empty AudioEndpoint prestate must be a valid zero-result observation",
    );
    invariant(
        prestate.includes("windows-labsn-prestate.txt"),
        "prestate evidence file is required",
    );

    invariant(action.includes(`uses: ${ACTION_PIN}`), "LABSN action must use the reviewed commit");
    invariant(action.includes("id: labsn_virtual_audio"), "LABSN action must expose its outcome");
    invariant(action.includes("continue-on-error: true"), "LABSN action must allow cleanup to run");
    invariant(!action.includes("with:"), "the pinned LABSN action has no inputs");

    invariant(
        cleanup.includes(`if: \${{ always() && ${LICENSE_CONDITION} }}`),
        "TrustedPublisher restoration must run under always()",
    );
    invariant(
        cleanup.includes("steps.labsn_virtual_audio.outcome") &&
            cleanup.includes("steps.labsn_prestate.outputs.publisher_match_count"),
        "cleanup must bind the action outcome and pre-action target state",
    );
    invariant(
        cleanup.includes("LABSN_VBCABLE_CERT_SHA256") &&
            cleanup.includes("Cert:\\LocalMachine\\TrustedPublisher") &&
            cleanup.includes("if ($before -eq '0')") &&
            cleanup.includes("certutil.exe -delstore 'TrustedPublisher'"),
        "cleanup must remove only the pinned certificate target",
    );
    invariant(
        cleanup.includes("publisher_state_restored=$restored") &&
            cleanup.includes("$publisherStateRestored.ToString().ToLowerInvariant()") &&
            cleanup.includes("publisher_cleanup_error=") &&
            cleanup.includes("publisher_removal_exits=") &&
            cleanup.includes("publisher_cleanup_error_stage=") &&
            cleanup.includes("windows-labsn-cleanup.txt"),
        "cleanup must prove and record restored certificate state",
    );
    invariant(
        cleanup.indexOf("windows-labsn-cleanup.txt") < cleanup.lastIndexOf("throw "),
        "cleanup evidence must be written before any terminal failure",
    );

    invariant(canary.includes(`if: \${{ ${LICENSE_CONDITION} }}`), "endpoint canary license gate drift");
    invariant(
        canary.includes("steps.labsn_virtual_audio.outcome") &&
            canary.includes("if ($env:LABSN_ACTION_OUTCOME -ne 'success')"),
        "endpoint canary must independently reject action failure",
    );
    invariant(
        canary.includes("$cableEndpoints.Count -lt 2") &&
            canary.includes("CABLE Input") &&
            canary.includes("'^(CABLE Input|Speakers) \\(VB-Audio Virtual Cable\\)$'") &&
            canary.includes("'^CABLE Output \\(VB-Audio Virtual Cable\\)$'") &&
            canary.includes("$hardwareIds -match 'VBAudioVACWDM'") &&
            canary.includes("Start-Service -Name Audiosrv") &&
            canary.includes("[TimeSpan]::FromSeconds(20)") &&
            canary.includes("Get-Service -Name Audiosrv") &&
            canary.includes("$audioService.Status -ne 'Running'"),
        "endpoint canary must prove the expected device, both endpoints, and audio service",
    );
    invariant(
        canary.includes("$property = Get-PnpDeviceProperty") &&
            canary.includes("$property.PSObject.Properties.Name -contains 'Data'") &&
            canary.includes("$hardwareIds = @($property.Data) -join ';'") &&
            !canary.includes("Select-Object -ExpandProperty Data"),
        "missing PnP hardware-ID properties must be a safe empty observation",
    );
    invariant(
        canary.includes("archive_integrity_verified_by_caller=false") &&
            canary.includes("catalog_signature_verified_by_caller=false") &&
            canary.includes("catalog_members_verified_by_caller=false") &&
            canary.includes("devcon_signature_verified_by_caller=false"),
        "supply-chain evidence must not claim checks the direct action does not perform",
    );
    invariant(
        canary.includes("action_inputs=none") && canary.includes("action_outputs=none"),
        "evidence must describe the pinned action interface honestly",
    );
    invariant(
        canary.includes("setup_proof_claimed=false") &&
            canary.includes("setup_proof_artifact=windows-installation-canary.txt") &&
            !canary.includes("setup_proof=post_action"),
        "pre-canary boundary must not claim installation proof",
    );
    invariant(
        canary.includes("excluded_claims=capture,playback,default_device,roundtrip,rsac") &&
            canary.includes("windows-supply-chain-boundary.txt") &&
            canary.includes("windows-endpoints.json") &&
            canary.includes("windows-installation-canary.txt"),
        "endpoint evidence must remain bounded and artifact-backed",
    );
    invariant(
        canary.includes("status=PASS") &&
            canary.includes("proof=post_action_vb_cable_device_and_endpoint_presence") &&
            canary.includes("evidence_class=device_and_endpoint_enumeration_only_no_pcm") &&
            canary.includes(
                'Set-Content -Path "$env:EVIDENCE_DIR/windows-installation-canary.txt"',
            ) &&
            canary.indexOf("status=PASS") > canary.indexOf("$audioService.Status -ne 'Running'") &&
            canary.indexOf("status=PASS") > canary.indexOf("windows-endpoints.json"),
        "affirmative canary proof must be emitted only after every presence check and inventory write",
    );
    invariant(
        canary.indexOf("windows-endpoints.json") < canary.indexOf("if ($endpoints.Count -eq 0)") &&
            canary.indexOf("windows-endpoints.json") <
                canary.indexOf("if ($cableEndpoints.Count -lt 2") &&
            canary.indexOf("windows-endpoints.json") <
                canary.indexOf("if ($vbCableDevices.Count -eq 0)") &&
            canary.indexOf("windows-endpoints.json") <
                canary.indexOf("if ($audioService.Status -ne 'Running')"),
        "observed endpoint/device inventory must be written before validation throws",
    );

    const forbidden = [
        "Check out pinned LABSN Windows helper assets",
        "VBCABLE_ARCHIVE_SHA256",
        "LABSN_DEVCON_SHA256",
        "Invoke-WebRequest",
        "Expand-Archive",
        "& $devcon install",
        "catalog_signature_verified=true",
        "archive_hash_verified_before_certificate_import=true",
    ];
    for (const token of forbidden) {
        invariant(!source.includes(token), `forbidden manual/provenance token remains: ${token}`);
    }

    invariant(
        durability.includes("canonical_durability"),
        "Windows durability command contract drift",
    );
    invariant(source.includes("expected_durability_tests: 47"), "Linux durability count drift");
    invariant(source.includes("expected_durability_tests: 17"), "macOS durability count drift");
    invariant(
        windowsMatrix.includes("expected_cc9a_native_tests: 1") &&
            windowsMatrix.includes("expected_durability_tests: 14") &&
            windowsMatrix.includes("expected_crash_harness_tests: 9"),
        "Windows cc9a/canonical/crash matrix row drift",
    );
    invariant(source.includes("expected_crash_harness_tests: 11"), "Unix crash count drift");
    invariant(source.includes("expected_crash_harness_tests: 9"), "Windows crash count drift");

    invariant(
        source.includes("expected_cc9a_native_tests: 3") &&
            source.includes("expected_cc9a_native_tests: 1") &&
            source.includes("EXPECTED_CC9A_NATIVE_TESTS: ${{ matrix.expected_cc9a_native_tests }}"),
        "cc9a native platform count/export drift",
    );
    for (const step of [cc9aUnix, cc9aWindows]) {
        invariant(
            step.includes("features cloud cc9a_native_ --") &&
                step.includes("cc9a_native.log") &&
                step.includes("cc9a_native.exit") &&
                step.includes("cc9a_native.tee.exit") &&
                step.includes("cc9a_native.tests") &&
                step.includes("cc9a_native.names"),
            "cc9a command or evidence-file contract drift",
        );
    }
    invariant(
        cc9aUnix.includes("pipeline_status") &&
            cc9aUnix.includes('pending_name=""') &&
            cc9aUnix.includes('$4 == "ok" && NF == 4') &&
            cc9aUnix.includes('$4 == "CC9A_MACOS_DIAGNOSTIC"') &&
            cc9aUnix.includes('pending_name != "" && $0 == "ok"'),
        "Unix cc9a exit or split-diagnostic exact-name capture drift",
    );
    invariant(
        cc9aWindows.includes("$LASTEXITCODE") &&
            cc9aWindows.includes("Select-String") &&
            cc9aWindows.includes("cc9a_native_[^ ]+"),
        "Windows cc9a exit or exact-name marker capture drift",
    );
    invariant(
        macosDiagnostics.includes("if: ${{ always() && matrix.os == 'macos' }}") &&
            macosDiagnostics.includes("macos-mount-diagnostics.txt") &&
            macosDiagnostics.includes("diagnostics_complete=true"),
        "macOS mount diagnostics must remain always-safe and artifact-backed",
    );
    invariant(
        macosDiagnostics.includes("stat -f 'device=%d inode=%i flags=%f'") &&
            macosDiagnostics.includes('targets=("$probe" "/" "/System/Volumes/Data")') &&
            macosDiagnostics.includes('for target in "${targets[@]}"'),
        "macOS stat device/inode/flags evidence drift",
    );
    invariant(
        macosDiagnostics.includes("mount | awk") &&
            macosDiagnostics.includes('$3 == "/"') &&
            macosDiagnostics.includes('$3 == "/System/Volumes/Data"') &&
            macosDiagnostics.includes("mount_record="),
        "macOS relevant mount-record evidence drift",
    );
    invariant(
        macosDiagnostics.includes('targets=("$probe" "/" "/System/Volumes/Data")') &&
            macosDiagnostics.includes('for target in "${targets[@]}"') &&
            macosDiagnostics.includes('df_output="$(df -P "$target" 2>&1)"') &&
            macosDiagnostics.includes("awk 'NR == 2 { print $NF }'") &&
            macosDiagnostics.includes("resolved_mount=") &&
            macosDiagnostics.includes('/usr/sbin/diskutil info "$resolved_mount"') &&
            macosDiagnostics.includes("Device Node") &&
            macosDiagnostics.includes("Volume UUID") &&
            macosDiagnostics.includes("Volume Roles") &&
            macosDiagnostics.includes("File System Personality") &&
            macosDiagnostics.includes("Read-Only Volume") &&
            macosDiagnostics.includes("Internal") &&
            macosDiagnostics.includes("Removable Media"),
        "macOS directory-to-mount diskutil identity/policy evidence drift",
    );
    invariant(
        macosDiagnostics.includes("set -uo pipefail") &&
            !macosDiagnostics.includes("set -euo pipefail") &&
            macosDiagnostics.includes('diskutil_exit="$?"') &&
            macosDiagnostics.includes("diskutil_output=") &&
            macosDiagnostics.includes('failure_count="$((failure_count + 1))"'),
        "macOS per-target diagnostic collection must remain nonfatal and observable",
    );
    invariant(
        macosDiagnostics.includes("printf 'target_count=%s\\n'") &&
            macosDiagnostics.includes("printf 'resolved_count=%s\\n'") &&
            macosDiagnostics.includes("printf 'success_count=%s\\n'") &&
            macosDiagnostics.includes("printf 'failure_count=%s\\n'") &&
            macosDiagnostics.includes("printf 'diagnostics_complete=true\\n'"),
        "macOS diagnostic exact-count/completion artifact contract drift",
    );
    invariant(
        macosDiagnostics.indexOf("exit 0") >
            macosDiagnostics.indexOf("printf 'diagnostics_complete=true\\n'") &&
            source.indexOf(`      - name: ${macosDiagnosticsName}\n`) <
                source.indexOf(`      - name: ${cc9aUnixName}\n`),
        "macOS diagnostic collection must not skip the Rust qualification steps",
    );
    invariant(
        cc9aUnix.includes('marker=index($0, "CC9A_MACOS_DIAGNOSTIC ")') &&
            cc9aUnix.includes('marker == 1 ||') &&
            cc9aUnix.includes('$4 == "CC9A_MACOS_DIAGNOSTIC"') &&
            cc9aUnix.includes("print substr($0, marker)") &&
            cc9aUnix.includes("cc9a_macos_diagnostics.txt") &&
            cc9aUnix.includes('if [ "$RUNNER_OS" = "macOS" ]'),
        "macOS Rust diagnostic substring extraction artifact drift",
    );
    invariant(
        unixSummary.includes(`-e '${POSIX_CC9A_INVENTORY_AVAILABLE_SED}'`) &&
            unixSummary.includes(`-e '${POSIX_CC9A_INVENTORY_UNAVAILABLE_SED}'`) &&
            !unixSummary.includes(GNU_CC9A_INVENTORY_ALTERNATION_SED),
        "macOS inventory extraction must use portable BRE expressions without GNU alternation",
    );
    invariant(
        unixSummary.includes("diagnostic_root_field_uniqueness_count") &&
            unixSummary.includes('gsub(/canonical_root=/, "&") == 1') &&
            unixSummary.includes('gsub(/root_dev=/, "&") == 1') &&
            unixSummary.includes('gsub(/inventory_count=/, "&") == 1') &&
            unixSummary.includes('gsub(/root_fsid_result=/, "&") == 1') &&
            unixSummary.includes('[ "$diagnostic_root_field_uniqueness_count" = 1 ]'),
        "macOS root diagnostic field labels must each occur exactly once",
    );
    invariant(
        unixSummary.includes("cc9a_macos_diagnostics_present") &&
            unixSummary.includes("cc9a_macos_diagnostics_complete") &&
            unixSummary.includes("cc9a_macos_exact_mount_identity") &&
            unixSummary.includes("macos_mount_diagnostics_complete") &&
            unixSummary.includes("cc9a_macos_diagnostics_complete=true") &&
            unixSummary.includes("cc9a_macos_exact_mount_identity=true") &&
            unixSummary.includes("macos_mount_diagnostics_complete=true") &&
            unixSummary.includes('mount_target_count="$(sed -n') &&
            unixSummary.includes('mount_resolved_count="$(sed -n') &&
            unixSummary.includes('mount_success_count="$(sed -n') &&
            unixSummary.includes('mount_failure_count="$(sed -n') &&
            unixSummary.includes('[ "$mount_target_count" = 3 ]') &&
            unixSummary.includes('[ "$mount_resolved_count" = 3 ]') &&
            unixSummary.includes('[ "$mount_success_count" = 3 ]') &&
            unixSummary.includes('[ "$mount_failure_count" = 0 ]') &&
            unixSummary.includes(
                '[ "$(grep -c \'^stat_exit=0$\' "$EVIDENCE_DIR/macos-mount-diagnostics.txt" || true)" = 3 ]',
            ) &&
            unixSummary.includes("diagnostic_observation_count") &&
            unixSummary.includes("diagnostic_observation_schema_count") &&
            unixSummary.includes("diagnostic_inventory_count") &&
            unixSummary.includes("diagnostic_total_count") &&
            unixSummary.includes("diagnostic_exact_count") &&
            unixSummary.includes("root_equals_data=true") &&
            unixSummary.includes("root_differs_system=true") &&
            unixSummary.includes("same_root_fsid_count=1") &&
            unixSummary.includes("probe_unavailable_count=0") &&
            unixSummary.includes("root_before_after_stable=true") &&
            unixSummary.includes("selection_authority=fsid") &&
            unixSummary.includes("mounted_on_text_authority=false") &&
            unixSummary.includes('[ "$diagnostic_total_count" = 5 ]') &&
            unixSummary.includes('[ "$diagnostic_inventory_count" = 2 ]') &&
            unixSummary.includes('[ "$diagnostic_observation_count" = 2 ]') &&
            unixSummary.includes('[ "$diagnostic_observation_schema_count" = 2 ]') &&
            unixSummary.includes('[ "$diagnostic_root_count" = 1 ]') &&
            unixSummary.includes('[ "$diagnostic_summary_count" = 1 ]') &&
            unixSummary.includes('[ "$diagnostic_exact_count" = 1 ]') &&
            unixSummary.includes('[ "$cc9a_macos_exact_mount_identity" != true ]') &&
            unixSummary.includes(
                "printf 'cc9a_macos_exact_mount_identity=%s\\n' \"$cc9a_macos_exact_mount_identity\"",
            ) &&
            unixSummary.includes('if [ "$RUNNER_OS" = "macOS" ]') &&
            unixSummary.includes("status=FAIL"),
        "macOS summary diagnostic completeness/fail-closed gate drift",
    );

    const expectedNames = [
        "cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation",
        "cc9a_native_qualified_initial_cas_has_parent_barrier",
        "cc9a_native_qualified_replacement_refuses_foreign_open_head_before_temp_creation",
        "cc9a_native_windows_qualified_existing_root_refuses_before_manifest_coordination_or_temp_mutation",
    ];
    for (const name of expectedNames) {
        invariant(
            unixSummary.includes(name) || windowsSummary.includes(name),
            `cc9a summary exact-name gate missing: ${name}`,
        );
    }
    for (const summary of [unixSummary, windowsSummary]) {
        invariant(
            summary.includes("EXPECTED_CC9A_NATIVE_TESTS") &&
                summary.includes("cc9a_native_tests") &&
                summary.includes("cc9a_native_expected_tests") &&
                summary.includes("cc9a_native_name_markers") &&
                summary.includes("cc9a_native_artifacts") &&
                summary.includes("cc9a_native.names"),
            "cc9a summary name/count/artifact gate drift",
        );
    }

    const guardTest = rustTest(canonicalSource, expectedNames[0]);
    invariant(
        guardTest.includes('#[cfg(any(target_os = "linux", target_os = "macos"))]'),
        "cc9a recreated-root guard cfg must include Linux and macOS",
    );
    invariant(
        guardTest.includes("for_existing_managed_root") &&
            guardTest.includes("try_lock_exclusive_qualified") &&
            guardTest.includes("QualificationBindingMismatch") &&
            guardTest.includes("fs::rename") &&
            guardTest.includes("fs::create_dir_all") &&
            guardTest.includes("COORDINATION_FILE_NAME"),
        "cc9a recreated-root guard proof weakened",
    );
    invariant(
        guardTest.includes('#[cfg(target_os = "macos")]') &&
            guardTest.includes("emit_cc9a_macos_mount_diagnostics(&first_root);"),
        "cc9a macOS live diagnostic call missing from counted guard test",
    );
    invariant(
        canonicalSource.includes('#[cfg(all(test, target_os = "macos"))]\nfn emit_cc9a_macos_mount_diagnostics') &&
            canonicalSource.includes('const MARKER: &str = "CC9A_MACOS_DIAGNOSTIC";'),
        "cc9a macOS diagnostics must remain test-only with a stable marker",
    );
    invariant(
        canonicalSource.includes("root canonical_root={} root_dev={} inventory_count={}") &&
            canonicalSource.includes("canonical_root") &&
            canonicalSource.includes("root_dev"),
        "cc9a macOS diagnostic root identity fields drift",
    );
    invariant(
        canonicalSource.includes("inventory_count={}") &&
            canonicalSource.includes("observation index={index}") &&
            canonicalSource.includes("mount_path={}") &&
            canonicalSource.includes("filesystem_class={:?}") &&
            canonicalSource.includes("filesystem_string={}") &&
            canonicalSource.includes("metadata_result={}") &&
            canonicalSource.includes("dev={}") &&
            canonicalSource.includes("same_root_dev={}") &&
            canonicalSource.includes("read_only={}") &&
            canonicalSource.includes("removable={}"),
        "cc9a macOS diagnostic inventory fields drift",
    );
    invariant(
        canonicalSource.includes("metadata_unavailable_count={metadata_unavailable_count}") &&
            canonicalSource.includes("same_root_dev_count={same_root_dev_count}") &&
            [
                "root_missing",
                "zero_match_clean",
                "zero_match_with_unavailable",
                "unique",
                "ambiguous",
                "unique_then_validate_mismatch",
            ].every((branch) => canonicalSource.includes(`"${branch}"`)),
        "cc9a macOS diagnostic cardinality branches drift",
    );
    invariant(
        canonicalSource.includes("root_fsid_result={}") &&
            canonicalSource.includes("fsid_result={}") &&
            canonicalSource.includes("same_root_fsid={}") &&
            canonicalSource.includes("root_equals_data={}") &&
            canonicalSource.includes("root_differs_system={}") &&
            canonicalSource.includes("same_root_fsid_count={same_root_fsid_count}") &&
            canonicalSource.includes("probe_unavailable_count={probe_unavailable_count}") &&
            canonicalSource.includes("root_before_after_stable={}") &&
            canonicalSource.includes("selection_authority=fsid") &&
            canonicalSource.includes("mounted_on_text_authority=false"),
        "cc9a macOS exact mount diagnostic relationship markers drift",
    );
    invariant(
        (canonicalSource.match(/resolve_live_filesystem\(/g) ?? []).length === 3 &&
            canonicalSource.includes("CanonicalPlatform::MacOs =>") &&
            canonicalSource.includes("resolve_exact_macos_mount(namespace)") &&
            canonicalSource.includes("nix::sys::statfs::fstatfs(directory)") &&
            canonicalSource.includes("observation.live_mount.is_none()") &&
            canonicalSource.includes("observation.live_mount.as_ref() == Some(root_mount)") &&
            canonicalSource.includes("require_stable_live_mount(&before, &after)?") &&
            canonicalSource.includes("require_macos_handle_identity(&root_dir, &namespace.identity)?") &&
            canonicalSource.includes("require_macos_handle_identity(&refreshed_dir, &namespace.identity)?"),
        "cc9a shared safe exact-mount production resolver drift",
    );
    const macosResolver = rustFunction(canonicalSource, "resolve_exact_macos_mount");
    const macosObservationInitializers = [
        ...macosResolver.matchAll(/FilesystemObservation \{([\s\S]*?)\n        \}/g),
    ].map((match) => match[1]);
    invariant(
        macosObservationInitializers.length > 0 &&
            macosObservationInitializers.every((initializer) =>
                initializer.includes("#[cfg(test)]\n            volume,"),
            ) &&
            macosResolver.includes(
                "#[cfg(test)]\n        let volume = filesystem_identity(&metadata).volume;",
            ),
        "cc9a macOS filesystem observations must retain test-only volume identity",
    );
    invariant(
        cargoManifest.includes('[target.\'cfg(target_os = "macos")\'.dependencies]') &&
            cargoManifest.includes('nix = { version = "0.31.3", features = ["fs"] }'),
        "cc9a macOS-target-only nix fs dependency drift",
    );
    const audioGraphLockPackage = cargoLock.slice(
        cargoLock.indexOf('name = "audio-graph"'),
        cargoLock.indexOf("\n[[package]]", cargoLock.indexOf('name = "audio-graph"')),
    );
    invariant(
        audioGraphLockPackage.includes('"nix 0.31.3"') &&
            (cargoLock.match(/name = "nix"\nversion = "0\.31\.3"/g) ?? []).length === 1 &&
            cargoLock.includes(
                'checksum = "cf20d2fde8ff38632c426f1165ed7436270b44f199fc55284c38276f9db47c3d"',
            ),
        "cc9a locked nix root edge or package identity drift",
    );

    const exactMountTest = rustTest(
        canonicalSource,
        "macos_volume_group_selection_binds_logical_root_to_unique_data_volume",
    );
    invariant(
        exactMountTest.includes("assert_eq!(system.volume, data.volume)") &&
            exactMountTest.includes("Some(7)") &&
            exactMountTest.includes("Some(42)") &&
            exactMountTest.includes("select_exact_macos_mount") &&
            exactMountTest.includes("live_mount: None") &&
            exactMountTest.includes("read_only: true") &&
            exactMountTest.includes("removable: true") &&
            exactMountTest.includes('file_system: OsString::from("hfs")') &&
            exactMountTest.includes("LiveMountIdentity::Synthetic(43)") &&
            exactMountTest.includes("root.starts_with(&system.mount_point)") &&
            exactMountTest.includes("!root.starts_with(&data.mount_point)"),
        "cc9a pure exact-mount refusal/masking coverage drift",
    );

    const initialTest = rustTest(manifestSource, expectedNames[1]);
    const replacementTest = rustTest(manifestSource, expectedNames[2]);
    for (const test of [initialTest, replacementTest]) {
        invariant(
            test.includes('#[cfg(any(target_os = "linux", target_os = "macos"))]') &&
                test.includes("SessionArtifactManifestStore::qualified_existing_root"),
            "cc9a qualified manifest test cfg or production constructor drift",
        );
    }
    invariant(
        initialTest.includes("InitialSnapshotInstall") &&
            initialTest.includes("FileAndParentNamespace"),
        "cc9a initial CAS parent-barrier proof weakened",
    );
    invariant(
        replacementTest.includes("IdentityChanged") &&
            replacementTest.includes("MANIFEST_TEMP_FILE_NAME") &&
            replacementTest.includes("validated_bytes") &&
            replacementTest.includes("assert_eq!"),
        "cc9a replacement foreign-head/no-temp proof weakened",
    );

    const windowsTest = rustTest(manifestSource, expectedNames[3]);
    invariant(
        windowsTest.includes('#[cfg(target_os = "windows")]') &&
            windowsTest.includes("SessionArtifactManifestStore::qualified_existing_root") &&
            !windowsTest.includes("qualified_for_algorithm_test_platform") &&
            !windowsTest.includes("qualified_for_test"),
        "cc9a Windows proof must use the production qualification seam",
    );
    invariant(
        windowsTest.includes("ManifestStoreError::Qualification") &&
            windowsTest.includes("CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported") &&
            windowsTest.includes("platform: CanonicalPlatform::Windows"),
        "cc9a Windows typed refusal proof weakened",
    );
    invariant(
        windowsTest.includes("assert_eq!(after_entries, before_entries)") &&
            windowsTest.includes("COORDINATION_FILE_NAME") &&
            windowsTest.includes("MANIFEST_TEMP_FILE_NAME") &&
            windowsTest.includes("manifest_path") &&
            windowsTest.match(/assert!\([^;]*\.exists\(\)/g)?.length >= 3,
        "cc9a Windows entry-equality or no-mutation proof weakened",
    );
}

function expectRejected(label, mutate) {
    let rejected = false;
    try {
        validate(mutate(workflow));
    } catch {
        rejected = true;
    }
    invariant(rejected, `mutation was not rejected: ${label}`);
}

validate(workflow);

const mutations = [
    [
        "floating LABSN ref",
        (source) => source.replace(`uses: ${ACTION_PIN}`, "uses: LABSN/sound-ci-helpers@v1"),
    ],
    ["fictitious action input", (source) => source.replace(`uses: ${ACTION_PIN}`, `uses: ${ACTION_PIN}\n        with:\n          device: vbcable`)],
    ["cleanup loses always", (source) => source.replace(`if: \${{ always() && ${LICENSE_CONDITION} }}`, `if: \${{ ${LICENSE_CONDITION} }}`)],
    ["cleanup removal omitted", (source) => source.replace("certutil.exe -delstore 'TrustedPublisher'", "Write-Output 'skip cleanup'")],
    [
        "cleanup target branch unreachable",
        (source) => source.replace("if ($before -eq '0')", "if ($false)"),
    ],
    ["pre-existing endpoint admitted", (source) => source.replace("if ($cableEndpoints.Count -ne 0)", "if ($false)")],
    [
        "empty endpoint class made fatal",
        (source) => source.replace(
            "Get-PnpDevice -PresentOnly -ErrorAction Stop |\n              Where-Object Class -eq 'AudioEndpoint' |",
            "Get-PnpDevice -Class AudioEndpoint -PresentOnly -ErrorAction Stop |",
        ),
    ],
    ["one endpoint admitted", (source) => source.replace("$cableEndpoints.Count -lt 2", "$cableEndpoints.Count -lt 1")],
    [
        "Pack43 render alias omitted",
        (source) => source.replace("|Speakers", ""),
    ],
    [
        "capture alias widened",
        (source) => source.replace(
            "'^CABLE Output \\(VB-Audio Virtual Cable\\)$'",
            "'^CABLE Output'",
        ),
    ],
    [
        "hardware identity omitted",
        (source) => source.replace("$hardwareIds -match 'VBAudioVACWDM'", "$true"),
    ],
    [
        "missing hardware-ID property made fatal",
        (source) => source.replace(
            "$property.PSObject.Properties.Name -contains 'Data'",
            "$true",
        ),
    ],
    [
        "audio service not required",
        (source) => source.replace("$audioService.Status -ne 'Running'", "$false"),
    ],
    [
        "action outcome admitted",
        (source) => source.replaceAll(
            "$env:LABSN_ACTION_OUTCOME -ne 'success'",
            "$false",
        ),
    ],
    ["caller archive verification overclaimed", (source) => source.replace("archive_integrity_verified_by_caller=false", "archive_integrity_verified_by_caller=true")],
    [
        "pre-canary proof overclaimed",
        (source) => source.replace(
            "setup_proof_claimed=false",
            "setup_proof=post_action_device_and_endpoint_presence",
        ),
    ],
    [
        "PASS canary omitted",
        (source) => source.replace(
            'Set-Content -Path "$env:EVIDENCE_DIR/windows-installation-canary.txt"',
            'Set-Content -Path "$env:EVIDENCE_DIR/windows-installation-canary-removed.txt"',
        ),
    ],
    ["manual installer reintroduced", (source) => `${source}\n# Invoke-WebRequest\n`],
];

for (const [label, mutate] of mutations) {
    expectRejected(label, mutate);
}

const cc9aMutations = [
    [
        "cc9a recreated-root test renamed",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: mutateRustTest(
                canonical,
                "cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation",
                "cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation",
                "renamed_recreated_root_guard",
            ),
        }),
    ],
    [
        "cc9a recreated-root cfg narrowed",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: mutateRustTest(
                canonical,
                "cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation",
                '#[cfg(any(target_os = "linux", target_os = "macos"))]',
                '#[cfg(target_os = "linux")]',
            ),
        }),
    ],
    [
        "cc9a initial CAS test renamed",
        ({ manifest, ...rest }) => ({
            ...rest,
            manifest: mutateRustTest(
                manifest,
                "cc9a_native_qualified_initial_cas_has_parent_barrier",
                "cc9a_native_qualified_initial_cas_has_parent_barrier",
                "renamed_initial_cas",
            ),
        }),
    ],
    [
        "cc9a manifest cfg narrowed",
        ({ manifest, ...rest }) => ({
            ...rest,
            manifest: mutateRustTest(
                manifest,
                "cc9a_native_qualified_initial_cas_has_parent_barrier",
                '#[cfg(any(target_os = "linux", target_os = "macos"))]',
                '#[cfg(target_os = "linux")]',
            ),
        }),
    ],
    [
        "cc9a replacement test renamed",
        ({ manifest, ...rest }) => ({
            ...rest,
            manifest: mutateRustTest(
                manifest,
                "cc9a_native_qualified_replacement_refuses_foreign_open_head_before_temp_creation",
                "cc9a_native_qualified_replacement_refuses_foreign_open_head_before_temp_creation",
                "renamed_replacement_cas",
            ),
        }),
    ],
    [
        "cc9a Windows proof made synthetic",
        ({ manifest, ...rest }) => ({
            ...rest,
            manifest: mutateRustTest(
                manifest,
                "cc9a_native_windows_qualified_existing_root_refuses_before_manifest_coordination_or_temp_mutation",
                "SessionArtifactManifestStore::qualified_existing_root(&root)",
                "SessionArtifactManifestStore::qualified_for_algorithm_test_platform(&root, CanonicalPlatform::Windows)",
            ),
        }),
    ],
    [
        "cc9a Windows typed refusal weakened",
        ({ manifest, ...rest }) => ({
            ...rest,
            manifest: mutateRustTest(
                manifest,
                "cc9a_native_windows_qualified_existing_root_refuses_before_manifest_coordination_or_temp_mutation",
                "CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported",
                "CanonicalFilesystemQualificationError::IdentityUnavailable",
            ),
        }),
    ],
    [
        "cc9a Windows entry equality removed",
        ({ manifest, ...rest }) => ({
            ...rest,
            manifest: mutateRustTest(
                manifest,
                "cc9a_native_windows_qualified_existing_root_refuses_before_manifest_coordination_or_temp_mutation",
                "assert_eq!(after_entries, before_entries)",
                "assert!(!after_entries.is_empty())",
            ),
        }),
    ],
    [
        "cc9a Unix command filter drift",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Run cc9a native qualification filter (Unix)",
                "features cloud cc9a_native_ --",
                "features cloud renamed_cc9a_ --",
            ),
        }),
    ],
    [
        "cc9a Windows command filter drift",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Run cc9a native qualification filter (Windows)",
                "features cloud cc9a_native_ --",
                "features cloud renamed_cc9a_ --",
            ),
        }),
    ],
    [
        "cc9a count evidence file drift",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: source.replaceAll("cc9a_native.tests", "cc9a_native.count-drift"),
        }),
    ],
    [
        "cc9a summary name gate drift",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: source.replaceAll("cc9a_native_name_markers", "cc9a_name_gate_removed"),
        }),
    ],
    [
        "cc9a platform count drift",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: source.replace("expected_cc9a_native_tests: 1", "expected_cc9a_native_tests: 0"),
        }),
    ],
    [
        "broad canonical platform count drift",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: source.replace("expected_durability_tests: 47", "expected_durability_tests: 46"),
        }),
    ],
    [
        "Windows broad canonical count drift",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: source.replace(
                `          - os: windows
            runner: windows-2025
            expected_cc9a_native_tests: 1
            expected_durability_tests: 14
            expected_crash_harness_tests: 9`,
                `          - os: windows
            runner: windows-2025
            expected_cc9a_native_tests: 1
            expected_durability_tests: 13
            expected_crash_harness_tests: 9`,
            ),
        }),
    ],
    [
        "cc9a macOS diagnostic call removed",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: mutateRustTest(
                canonical,
                "cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation",
                "emit_cc9a_macos_mount_diagnostics(&first_root);",
                "let _ = &first_root;",
            ),
        }),
    ],
    [
        "cc9a macOS root identity field drift",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: canonical.replace("root_dev={}", "root_device={}"),
        }),
    ],
    [
        "cc9a macOS inventory field drift",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: canonical.replaceAll("inventory_count", "inventory_total"),
        }),
    ],
    [
        "cc9a macOS cardinality branch drift",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: canonical.replace('"ambiguous"', '"multiple"'),
        }),
    ],
    [
        "cc9a macOS exact relation marker drift",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: canonical.replace("root_equals_data={}", "root_matches_data={}"),
        }),
    ],
    [
        "cc9a candidate probe failure skipped",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: canonical.replace(
                "observation.live_mount.is_none()",
                "observation.live_mount.is_some()",
            ),
        }),
    ],
    [
        "cc9a descriptor-bound fstatfs removed",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: canonical.replace(
                "nix::sys::statfs::fstatfs(directory)",
                "nix::sys::statfs::statfs(Path::new(\"/\"))",
            ),
        }),
    ],
    [
        "cc9a macOS filesystem observation volume identity removed",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: canonical.replace(
                "            #[cfg(test)]\n            volume,\n            live_mount: Some(macos_live_mount_identity(&mount_dir)?),",
                "            live_mount: Some(macos_live_mount_identity(&mount_dir)?),",
            ),
        }),
    ],
    [
        "cc9a before-after mount stability removed",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: canonical.replace(
                "require_stable_live_mount(&before, &after)?;",
                "let _ = (&before, &after);",
            ),
        }),
    ],
    [
        "cc9a mounted-on masking coverage removed",
        ({ canonical, ...rest }) => ({
            ...rest,
            canonical: mutateRustTest(
                canonical,
                "macos_volume_group_selection_binds_logical_root_to_unique_data_volume",
                "assert!(!root.starts_with(&data.mount_point));",
                "assert!(root.starts_with(&system.mount_point));",
            ),
        }),
    ],
    [
        "cc9a macOS exact PASS relation weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                "root_equals_data=true",
                "root_equals_data=(true|false)",
            ),
        }),
    ],
    [
        "cc9a macOS exact count gate omitted",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$diagnostic_exact_count" = 1 ]',
                '[ -n "$diagnostic_exact_count" ]',
            ),
        }),
    ],
    [
        "cc9a macOS exact status gate omitted",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$cc9a_macos_exact_mount_identity" != true ]',
                '[ "$cc9a_macos_exact_mount_identity" = missing ]',
            ),
        }),
    ],
    [
        "macOS stat evidence removed",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Record macOS mount diagnostics",
                "stat -f 'device=%d inode=%i flags=%f'",
                "printf 'stat unavailable'",
            ),
        }),
    ],
    [
        "macOS mount evidence removed",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Record macOS mount diagnostics",
                "mount | awk",
                "printf 'mount unavailable' | awk",
            ),
        }),
    ],
    [
        "macOS diskutil evidence removed",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Record macOS mount diagnostics",
                '/usr/sbin/diskutil info "$resolved_mount"',
                "printf 'diskutil unavailable'",
            ),
        }),
    ],
    [
        "macOS Rust diagnostic extraction artifact drift",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: source.replaceAll(
                "cc9a_macos_diagnostics.txt",
                "cc9a_macos_diagnostics-missing.txt",
            ),
        }),
    ],
    [
        "macOS inline Rust diagnostic context omitted",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Run cc9a native qualification filter (Unix)",
                "marker == 1 ||",
                "marker == 1 &&",
            ),
        }),
    ],
    [
        "cc9a split diagnostic failure admitted as a passed name",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Run cc9a native qualification filter (Unix)",
                'pending_name != "" && $0 == "ok"',
                'pending_name != "" && $0 != "ok"',
            ),
        }),
    ],
    [
        "macOS summary completeness weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                "cc9a_macos_diagnostics_complete=true",
                "cc9a_macos_diagnostics_complete=unchecked",
            ),
        }),
    ],
    [
        "macOS diagnostic total count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$diagnostic_total_count" = 5 ]',
                '[ -n "$diagnostic_total_count" ]',
            ),
        }),
    ],
    [
        "macOS inventory parser regressed to GNU BRE alternation",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                `-e '${POSIX_CC9A_INVENTORY_AVAILABLE_SED}' \\\n                -e '${POSIX_CC9A_INVENTORY_UNAVAILABLE_SED}'`,
                `'${GNU_CC9A_INVENTORY_ALTERNATION_SED}'`,
            ),
        }),
    ],
    [
        "macOS diagnostic inventory count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$diagnostic_inventory_count" = 2 ]',
                '[ -n "$diagnostic_inventory_count" ]',
            ),
        }),
    ],
    [
        "macOS diagnostic observation count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$diagnostic_observation_count" = 2 ]',
                '[ "$diagnostic_observation_count" = "$diagnostic_inventory_count" ]',
            ),
        }),
    ],
    [
        "macOS diagnostic observation schema count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$diagnostic_observation_schema_count" = 2 ]',
                '[ "$diagnostic_observation_schema_count" = "$diagnostic_inventory_count" ]',
            ),
        }),
    ],
    [
        "macOS diagnostic root count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$diagnostic_root_count" = 1 ]',
                '[ -n "$diagnostic_root_count" ]',
            ),
        }),
    ],
    ...["canonical_root", "root_dev", "inventory_count", "root_fsid_result"].map((field) => [
        `macOS root ${field} uniqueness check removed`,
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                `gsub(/${field}=/, "&") == 1`,
                `gsub(/${field}=/, "&") >= 1`,
            ),
        }),
    ]),
    [
        "macOS root field uniqueness count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$diagnostic_root_field_uniqueness_count" = 1 ]',
                '[ -n "$diagnostic_root_field_uniqueness_count" ]',
            ),
        }),
    ],
    [
        "macOS diagnostic summary count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$diagnostic_summary_count" = 1 ]',
                '[ -n "$diagnostic_summary_count" ]',
            ),
        }),
    ],
    [
        "macOS directory-to-mount resolution removed",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Record macOS mount diagnostics",
                'df_output="$(df -P "$target" 2>&1)"',
                'df_output="$target"',
            ),
        }),
    ],
    [
        "macOS diskutil handling made fail-fast",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Record macOS mount diagnostics",
                "set -uo pipefail",
                "set -euo pipefail",
            ),
        }),
    ],
    [
        "macOS exact diagnostic count omitted",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Record macOS mount diagnostics",
                "printf 'resolved_count=%s\\n'",
                "printf 'resolved_total=%s\\n'",
            ),
        }),
    ],
    [
        "macOS diagnostic step can skip Rust tests",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Record macOS mount diagnostics",
                "exit 0",
                "exit 1",
            ),
        }),
    ],
    [
        "macOS summary success count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$mount_success_count" = 3 ]',
                '[ "$mount_success_count" = 2 ]',
            ),
        }),
    ],
    [
        "macOS summary stat success count weakened",
        ({ workflow: source, ...rest }) => ({
            ...rest,
            workflow: mutateStep(
                source,
                "Summarize and enforce native exits (Unix)",
                '[ "$(grep -c \'^stat_exit=0$\' "$EVIDENCE_DIR/macos-mount-diagnostics.txt" || true)" = 3 ]',
                '[ "$(grep -c \'^stat_exit=0$\' "$EVIDENCE_DIR/macos-mount-diagnostics.txt" || true)" = 2 ]',
            ),
        }),
    ],
];

for (const [label, mutate] of cc9aMutations) {
    let rejected = false;
    try {
        const candidate = mutate({
            workflow,
            canonical: canonicalDurability,
            manifest: sessionArtifactManifest,
        });
        validate(candidate.workflow, candidate.canonical, candidate.manifest);
    } catch {
        rejected = true;
    }
    invariant(rejected, `mutation was not rejected: ${label}`);
}

const mutationCount = mutations.length + cc9aMutations.length;
const statFailureArtifact = macosMountSummarySimulation([0, 1, 0]);
const fullGoodArtifact = macosMountSummarySimulation([0, 0, 0]);
const expectedLiveNames = [
    "cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation",
    "cc9a_native_qualified_initial_cas_has_parent_barrier",
    "cc9a_native_qualified_replacement_refuses_foreign_open_head_before_temp_creation",
].sort();
const priorLiveDiagnostics = priorCc9aMacosDiagnostics(CC9A_LIVE_INLINE_LOG);
const correctedLiveDiagnostics = correctedCc9aMacosDiagnostics(CC9A_LIVE_INLINE_LOG);
const priorLiveNames = priorCc9aNativeNames(CC9A_LIVE_INLINE_LOG);
const correctedLiveNames = correctedCc9aNativeNames(CC9A_LIVE_INLINE_LOG);
const failedSplitLog = CC9A_LIVE_INLINE_LOG.replace("\nok\n", "\nFAILED\n");
const noisyLiveLog = `${CC9A_LIVE_INLINE_LOG}\nerror: ${CC9A_MACOS_DIAGNOSTIC_MARKER}not-test-output`;
const liveDiagnosticArtifact = correctedLiveDiagnostics.join("\n");
const sixthDiagnosticArtifact = `${liveDiagnosticArtifact}\n${CC9A_MACOS_DIAGNOSTIC_MARKER}unexpected valid_prefix=true`;
const thirdObservation =
    "CC9A_MACOS_DIAGNOSTIC observation index=2 mount_path=/Volumes/Other filesystem_class=Apfs filesystem_string=apfs metadata_result=available dev=16777230 same_root_dev=false fsid_result=available same_root_fsid=false read_only=false removable=false";
const inventoryThreeArtifact = liveDiagnosticArtifact
    .replace("inventory_count=2", "inventory_count=3")
    .replace(
        `${CC9A_MACOS_DIAGNOSTIC_MARKER}summary`,
        `${thirdObservation}\n${CC9A_MACOS_DIAGNOSTIC_MARKER}summary`,
    );
const archived31994090474RootDiagnostic =
    "CC9A_MACOS_DIAGNOSTIC root canonical_root=/private/var/folders/pm/cmklcsfj60nd7nfc79g8xmbc0000gn/T/ag-canonical-production-binding-first-12520-0 root_dev=16777227 inventory_count=2 root_fsid_result=available";
const malformedArchivedRootDiagnostic = archived31994090474RootDiagnostic.replace(
    "inventory_count=2",
    "inventory_count=two",
);
const duplicateArchivedRootDiagnostics = [
    archived31994090474RootDiagnostic,
    archived31994090474RootDiagnostic.replace(
        "root_fsid_result=available",
        "root_fsid_result=unavailable",
    ),
].join("\n");
const duplicateFieldRootArtifact = liveDiagnosticArtifact.replace(
    "inventory_count=2 root_fsid_result=available",
    "inventory_count=999 root_fsid_result=unavailable inventory_count=2 root_fsid_result=available",
);
invariant(
    priorLiveDiagnostics.length === 4 &&
        !priorLiveDiagnostics.some((line) => line.startsWith(`${CC9A_MACOS_DIAGNOSTIC_MARKER}root `)),
    "inline diagnostic RED fixture no longer reproduces the anchored root omission",
);
invariant(
    correctedLiveDiagnostics.length === 5 &&
        correctedLiveDiagnostics[0].startsWith(`${CC9A_MACOS_DIAGNOSTIC_MARKER}root `) &&
        correctedCc9aMacosDiagnostics(noisyLiveLog).length === 5,
    "inline diagnostic GREEN must extract the valid substring without admitting unrelated text",
);
invariant(
    priorLiveNames.length === 2 && !priorLiveNames.includes(expectedLiveNames[0]),
    "split test-name RED fixture no longer reproduces the canonical-name omission",
);
invariant(
    JSON.stringify(correctedLiveNames) === JSON.stringify(expectedLiveNames),
    "split test-name GREEN must recover the exact three live names",
);
invariant(
    !correctedCc9aNativeNames(failedSplitLog).includes(expectedLiveNames[0]),
    "split test-name GREEN must not admit a standalone FAILED result",
);
invariant(
    priorCc9aMacosDiagnosticSummaryAccepts(sixthDiagnosticArtifact),
    "diagnostic-cardinality RED fixture no longer admits a sixth valid-prefix marker",
);
invariant(
    priorCc9aMacosDiagnosticSummaryAccepts(inventoryThreeArtifact),
    "diagnostic-cardinality RED fixture no longer admits inventory=3 with three observations",
);
invariant(
    correctedCc9aMacosDiagnosticSummaryAccepts(liveDiagnosticArtifact) &&
        !correctedCc9aMacosDiagnosticSummaryAccepts(sixthDiagnosticArtifact) &&
        !correctedCc9aMacosDiagnosticSummaryAccepts(inventoryThreeArtifact),
    "diagnostic-cardinality GREEN must accept exact 5/2/2/2/1/1/1 and reject both false passes",
);
invariant(
    correctedCc9aMacosDiagnosticSummaryAccepts(duplicateFieldRootArtifact),
    "duplicate-field RED fixture no longer reproduces the current exact-count false pass",
);
invariant(
    fieldUniqueCc9aMacosDiagnosticSummaryAccepts(liveDiagnosticArtifact) &&
        !fieldUniqueCc9aMacosDiagnosticSummaryAccepts(duplicateFieldRootArtifact),
    "root-field uniqueness GREEN must accept the archive shape and reject duplicate labels",
);
invariant(
    posixCc9aInventoryValues(archived31994090474RootDiagnostic).join("\n") === "2" &&
        posixCc9aInventorySummaryAccepts(archived31994090474RootDiagnostic) &&
        !posixCc9aInventorySummaryAccepts(malformedArchivedRootDiagnostic) &&
        !posixCc9aInventorySummaryAccepts(duplicateArchivedRootDiagnostics),
    "POSIX inventory extraction must return exact archived value 2 and reject malformed or multiple values",
);
invariant(
    priorMacosMountSummaryAccepts(statFailureArtifact),
    "summary simulation RED fixture no longer reproduces the prior false PASS",
);
invariant(
    !correctedMacosMountSummaryAccepts(statFailureArtifact),
    "summary simulation GREEN must reject one failed stat observation",
);
invariant(
    correctedMacosMountSummaryAccepts(fullGoodArtifact),
    "summary simulation full-good artifact must PASS",
);
console.log(
    "PASS: macOS summary simulation prior_false_pass=true corrected_failure_rejected=true full_good_pass=true",
);
console.log(
    "PASS: cc9a live inline simulation anchored_root_omitted=true split_name_omitted=true corrected_exact=true failed_rejected=true",
);
console.log(
    "PASS: cc9a diagnostic cardinality prior_sixth_pass=true prior_inventory3_pass=true exact_5_2_2_2_1_1_1=true extras_rejected=true",
);
console.log(
    "PASS: cc9a POSIX inventory extraction archived_31994090474=2 malformed_rejected=true multiple_rejected=true gnu_bre_alternation_rejected=true",
);
console.log(
    "PASS: cc9a root-field uniqueness prior_duplicate_fields_pass=true exact_once_required=true duplicate_fields_rejected=true",
);
console.log(`PASS: direct LABSN and cc9a native evidence contract with ${mutationCount} mutations`);
