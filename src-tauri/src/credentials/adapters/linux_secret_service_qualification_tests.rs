use super::*;
use crate::credentials::adapters::native_interaction::NativeInteractionGate;
use std::io::Write;

const OPT_IN: &str = "AUDIOGRAPH_QUALIFY_SECRET_SERVICE";
const CHILD_ROOT: &str = "AUDIOGRAPH_SECRET_SERVICE_TEST_ROOT";

#[test]
#[ignore = "requires the explicit headless GNOME Keyring qualification opt-in"]
fn gnome_keyring_headless() {
    assert!(
        std::env::var(OPT_IN).is_ok_and(|value| value == "1"),
        "set {OPT_IN}=1 to qualify GNOME Keyring"
    );
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        qualify(PathBuf::from(root));
        return;
    }

    let bus = Bus::configure();
    let config = bus.root.join("session.conf");
    let socket = bus.root.join("qualification.sock");
    let socket_xml = socket
        .to_string_lossy()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let private_config = fs::read_to_string(&config).unwrap().replace(
        "<listen>unix:tmpdir=/tmp</listen>",
        &format!("<listen>unix:path={socket_xml}</listen>"),
    );
    fs::write(&config, private_config).unwrap();
    let mut child = Command::new("timeout");
    child
        .args(["60s", "dbus-run-session"])
        .arg(format!("--config-file={}", config.display()))
        .arg("--")
        .arg(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "--nocapture",
            concat!(module_path!(), "::gnome_keyring_headless")
                .split_once("::")
                .unwrap()
                .1,
        ])
        .env(CHILD_ROOT, &bus.root)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("GNOME_KEYRING_CONTROL");
    for name in [
        "HOME",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        use std::os::unix::fs::DirBuilderExt;
        let dir = bus.root.join(name);
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        child.env(name, dir);
    }
    let status = child
        .status()
        .expect("launch the isolated dbus-run-session qualification");
    assert!(
        status.success() && bus.root.join("qualified").is_file(),
        "headless Secret Service qualification did not complete"
    );
    assert!(!bus.activation_attempted(), "client activated the canary");
}

fn qualify(root: PathBuf) {
    let isolated_address = format!("unix:path={}", root.join("qualification.sock").display());
    let address = session_address().expect("private D-Bus address");
    assert!(
        address.to_string().split(',').next() == Some(isolated_address.as_str()),
        "qualification must use its own bus"
    );
    let attributes = wanted();
    assert_eq!(
        read_at(address.clone(), &attributes, DEADLINE)
            .unwrap_err()
            .0,
        Failure::Unavailable
    );
    assert!(
        !root.join("started").exists(),
        "client activated the canary"
    );

    let _daemon =
        start_daemon(address.clone()).expect("GNOME Keyring did not own the private service");
    let gate = NativeInteractionGate::isolated_for_test();
    let mut lease = gate.acquire().unwrap();
    let (permission, invocation) = lease.mutation_capabilities();
    let mut mutate = |mutation| {
        mutations::mutate_at(
            address.clone(),
            permission,
            invocation,
            &attributes,
            mutation,
            Duration::from_secs(10),
            None,
        )
    };
    let write = |value: &[u8]| mutations::Mutation::Write(Zeroizing::new(value.to_vec()));
    let read = || read_at(address.clone(), &attributes, DEADLINE);

    assert!(read().unwrap().is_none());
    for value in [
        b"create fixture".as_slice(),
        b"replacement fixture".as_slice(),
    ] {
        assert_eq!(mutate(write(value)), Ok(MutationOutcome::Committed));
        assert!(
            read()
                .unwrap()
                .is_some_and(|secret| secret.0.as_slice() == value)
        );
    }
    assert_eq!(
        mutate(mutations::Mutation::Delete),
        Ok(MutationOutcome::Committed)
    );
    assert!(read().unwrap().is_none());

    assert_eq!(
        mutate(write(b"lock fixture")),
        Ok(MutationOutcome::Committed)
    );
    drive(DEADLINE, async |end| {
        until(end, async {
            let client = Client::connect(address.clone()).await?;
            let collection = client.default_collection().await?.ok_or(Failure::Missing)?;
            let objects = vec![collection.clone()];
            let (locked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = client
                .service_call("Lock", &(objects,))
                .await
                .expect("daemon accepts the qualification lock request");
            assert!(prompt.as_str() == "/" && locked.contains(&collection));
            assert!(client.locked(&collection, super::super::COLLECTION).await?);
            Ok(())
        })
        .await
    })
    .expect("lock the isolated collection without a prompt");
    assert_eq!(read().unwrap_err().0, Failure::Locked);
    assert_eq!(
        mutate(write(b"refused fixture")).unwrap_err().0,
        Failure::Locked
    );
    assert!(
        !root.join("started").exists(),
        "client activated the canary"
    );
    fs::write(root.join("qualified"), b"ok").unwrap();
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_daemon(address: Address) -> WireResult<Daemon> {
    drive(GUARD, async |end| {
        until(end, async {
            let bus = Builder::address(address).build().await.map_err(map_error)?;
            let rule = zbus::MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(BUS)
                .path(BUS_PATH)
                .interface(BUS)
                .member("NameOwnerChanged")
                .arg(0, SERVICE_NAME)
                .build()
                .map_err(map_error)?;
            let mut ownership = MessageStream::for_match_rule(rule, &bus, None)
                .await
                .map_err(map_error)?;
            let mut daemon = Daemon(
                Command::new("gnome-keyring-daemon")
                    .args(["--foreground", "--components=secrets", "--unlock"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("opted-in qualification requires gnome-keyring-daemon on PATH"),
            );
            daemon
                .0
                .stdin
                .take()
                .unwrap()
                .write_all(b"qualification fixture passphrase")
                .expect("initialize isolated login keyring");
            let change = ownership
                .next()
                .await
                .ok_or(Failure::Unavailable)?
                .map_err(map_error)?;
            let (_, _, owner): (String, String, OwnedUniqueName) =
                change.body().deserialize().map_err(map_error)?;
            let pid: u32 = flagged_call(
                &bus,
                BUS,
                BUS_PATH,
                BUS,
                "GetConnectionUnixProcessID",
                &(owner.as_str(),),
            )
            .await?;
            assert_eq!(
                pid,
                daemon.0.id(),
                "the opted-in daemon must own the service"
            );
            Ok(daemon)
        })
        .await
    })
}

#[test]
fn recovery_method_and_gate_source_inventory_is_closed() {
    let source = std::path::Path::new(file!());
    let source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(source)
    };
    let directory = source.parent().unwrap();
    let methods = [
        format!("\"{}\"", ["Un", "lock"].concat()),
        format!("\"{}\"", ["Pro", "mpt"].concat()),
    ];
    let mut found_recovery = false;
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let name = path.file_name().unwrap().to_str().unwrap();
        if !name.starts_with("linux_secret_service_") || !name.ends_with(".rs") {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        if name != "linux_secret_service_recovery.rs" {
            for method in &methods {
                assert!(
                    !source.contains(method),
                    "interactive method outside recovery: {name}"
                );
            }
            continue;
        }
        found_recovery = true;
        for method in &methods {
            assert_eq!(source.matches(method).count(), 1);
        }
        let production_and_test_seam: String =
            source.chars().filter(|ch| !ch.is_whitespace()).collect();
        for signature in [
            "NativeInteractionGate{",
            "NativeInteractionGate::",
            "AllowPrompt{",
            "ForbidPrompt{",
            "::mint(",
            ".access",
            ".stalled",
            ".lease",
            "latch_commit_unknown(",
            "latch_forbid_uncertainty(",
        ] {
            assert!(
                !production_and_test_seam.contains(signature),
                "recovery uses a sealed gate implementation detail: {signature}"
            );
        }
        assert_eq!(
            source.matches("NativeInteractionGate").count(),
            1,
            "the only gate type use is test injection"
        );
        assert!(source.contains("let gate = process_native_interaction_gate();"));
    }
    assert!(found_recovery);
}
