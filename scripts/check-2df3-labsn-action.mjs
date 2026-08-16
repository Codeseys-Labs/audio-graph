import { readFileSync } from "node:fs";
import process from "node:process";

const workflowPath = new URL(
    "../.github/workflows/2df3-native-durability.yml",
    import.meta.url,
);
const workflow = readFileSync(workflowPath, "utf8");

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

function validate(source) {
    const prestateName = "Record LABSN Windows prestate";
    const actionName = "Install Windows virtual audio baseline with pinned LABSN action";
    const cleanupName = "Restore LABSN TrustedPublisher state";
    const canaryName = "Record bounded allowlisted Windows endpoint inventory";
    const durabilityName = "Run canonical durability filter (Windows)";

    const prestate = stepBody(source, prestateName);
    const action = stepBody(source, actionName);
    const cleanup = stepBody(source, cleanupName);
    const canary = stepBody(source, canaryName);
    const durability = stepBody(source, durabilityName);

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
        durability.includes("expected_durability_tests") ||
            source.includes("expected_durability_tests: 12"),
        "Windows durability command contract drift",
    );
    invariant(source.includes("expected_durability_tests: 42"), "Linux durability count drift");
    invariant(source.includes("expected_durability_tests: 13"), "macOS durability count drift");
    invariant(source.includes("expected_durability_tests: 12"), "Windows durability count drift");
    invariant(source.includes("expected_crash_harness_tests: 11"), "Unix crash count drift");
    invariant(source.includes("expected_crash_harness_tests: 9"), "Windows crash count drift");
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

console.log(`PASS: direct LABSN action contract and ${mutations.length} mutations`);
