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

const ACTION_PIN =
    "LABSN/sound-ci-helpers@d08c889a7bba7d9b1b059f8f76dac4672ea3a9cf";
const LICENSE_CONDITION =
    "matrix.os == 'windows' && inputs.confirm_vb_cable_professional_license == true";

function invariant(condition, message) {
    if (!condition) {
        throw new Error(message);
    }
}

function stepBody(source, name) {
    const marker = `      - name: ${name}\n`;
    const start = source.indexOf(marker);
    invariant(start >= 0, `missing workflow step: ${name}`);
    const next = source.indexOf("\n      - name: ", start + marker.length);
    return source.slice(start, next < 0 ? source.length : next);
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

function validate(source, canonicalSource = canonicalDurability, manifestSource = sessionArtifactManifest) {
    const prestateName = "Record LABSN Windows prestate";
    const actionName = "Install Windows virtual audio baseline with pinned LABSN action";
    const cleanupName = "Restore LABSN TrustedPublisher state";
    const canaryName = "Record bounded allowlisted Windows endpoint inventory";
    const durabilityName = "Run canonical durability filter (Windows)";
    const cc9aUnixName = "Run cc9a native qualification filter (Unix)";
    const cc9aWindowsName = "Run cc9a native qualification filter (Windows)";

    const prestate = stepBody(source, prestateName);
    const action = stepBody(source, actionName);
    const cleanup = stepBody(source, cleanupName);
    const canary = stepBody(source, canaryName);
    const durability = stepBody(source, durabilityName);
    const cc9aUnix = stepBody(source, cc9aUnixName);
    const cc9aWindows = stepBody(source, cc9aWindowsName);
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
    invariant(source.includes("expected_durability_tests: 46"), "Linux durability count drift");
    invariant(source.includes("expected_durability_tests: 16"), "macOS durability count drift");
    invariant(source.includes("expected_durability_tests: 15"), "Windows durability count drift");
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
        cc9aUnix.includes("pipeline_status") && cc9aUnix.includes('$(NF) == "ok"'),
        "Unix cc9a exit or exact-name marker capture drift",
    );
    invariant(
        cc9aWindows.includes("$LASTEXITCODE") &&
            cc9aWindows.includes("Select-String") &&
            cc9aWindows.includes("cc9a_native_[^ ]+"),
        "Windows cc9a exit or exact-name marker capture drift",
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
            workflow: source.replace("expected_durability_tests: 46", "expected_durability_tests: 45"),
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
console.log(`PASS: direct LABSN and cc9a native evidence contract with ${mutationCount} mutations`);
