use super::*;
use crate::test_log_capture;
use futures_util::StreamExt;
use futures_util::future::{Either, select};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::pin::pin;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use tokio::sync::oneshot;
use zbus::connection::Builder;
use zbus::message::{Flags, Type};
use zbus::{Connection, Message, MessageStream};

pub(in crate::credentials::adapters) const GUARD: Duration = Duration::from_secs(15);
pub(in crate::credentials::adapters) const ITEM_PATH: &str =
    "/org/freedesktop/secrets/collection/locator_canary/first";
const OTHER_ITEM: &str = "/org/freedesktop/secrets/collection/locator_canary/other";
pub(in crate::credentials::adapters) const COLLECTION: &str =
    "/org/freedesktop/secrets/collection/locator_canary";
const SESSION: &str = "/org/freedesktop/secrets/session/locator_canary";
pub(in crate::credentials::adapters) const SECRET: &str = "secret canary marmalade";
pub(in crate::credentials::adapters) const ATTRIBUTE: &str = "attribute canary thyme";
pub(in crate::credentials::adapters) const LOCATOR: &str = "locator_canary";

pub(in crate::credentials::adapters) fn wanted() -> Attributes {
    attributes_for(&crate::credentials::adapters::keyring_entry::EntryLocator::authority())
}

pub(in crate::credentials::adapters) struct Bus {
    root: PathBuf,
    daemon: Option<Child>,
    address: String,
}

impl Bus {
    fn configure() -> Self {
        let root = std::env::temp_dir().join(format!("ag-bus-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).expect("create private bus directory");
        let bus = Self {
            root,
            daemon: None,
            address: String::new(),
        };
        let service_dir = bus.root.join("services");
        fs::create_dir(&service_dir).unwrap();
        let script = service_dir.join("canary.sh");
        fs::write(&script, "touch -- \"$1\"\nexit 1\n").unwrap();
        fs::write(
            service_dir.join(format!("{SERVICE_NAME}.service")),
            format!(
                "[D-BUS Service]\nName={SERVICE_NAME}\nExec=/bin/sh \"{}\" \"{}\"\n",
                script.display(),
                bus.root.join("started").display()
            ),
        )
        .unwrap();
        let config = bus.root.join("session.conf");
        fs::write(
            &config,
            format!(
                "<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>\
            <servicedir>{}</servicedir><policy context=\"default\">\
            <allow send_destination=\"*\"/><allow receive_sender=\"*\"/><allow own=\"*\"/>\
            </policy></busconfig>",
                service_dir
                    .to_string_lossy()
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            ),
        )
        .unwrap();
        bus
    }

    pub(in crate::credentials::adapters) fn start() -> Self {
        let mut bus = Self::configure();
        bus.daemon = Some(
            Command::new("dbus-daemon")
                .arg("--config-file")
                .arg(bus.root.join("session.conf"))
                .args(["--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("start private dbus-daemon"),
        );
        let stdout = bus.daemon.as_mut().unwrap().stdout.take().unwrap();
        let (ready, response) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut address = String::new();
            if BufReader::new(stdout).read_line(&mut address).is_ok() {
                let _ = ready.send(address.trim().to_owned());
            }
        });
        bus.address = response.recv_timeout(GUARD).expect("dbus-daemon ready");
        reader.join().unwrap();
        assert!(
            !bus.address.is_empty(),
            "private bus did not publish an address"
        );
        bus
    }

    pub(in crate::credentials::adapters) fn address(&self) -> Address {
        self.address.parse().expect("private bus address")
    }

    pub(in crate::credentials::adapters) fn activation_attempted(&self) -> bool {
        self.root.join("started").exists()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        if let Some(daemon) = &mut self.daemon {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::credentials::adapters) enum Scenario {
    Found,
    Absent,
    Superset,
    Locked,
    Duplicate,
    RelockBeforeSession,
    RelockAtGet,
    PlainOnly,
    PlainReply,
    WrongSession,
    AttributesChanged,
    SearchObjectMissing,
    SessionServiceObjectMissing,
    LockedItemMissing,
    UnlockedItemMissing,
    Disconnect,
    AliasMovesAfterSession,
    ServiceMovesAfterSession,
    Silent,
    SilentAtGet,
    CloseFails,
    Denied,
    UnknownError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::credentials::adapters) enum Call {
    Alias,
    Search,
    Attributes,
    Locked,
    CollectionLocked,
    Session,
    Secret,
    Close,
    Create,
    SetSecret,
    Delete,
    Dismiss,
}

#[derive(Clone, Copy)]
enum Identity {
    Gnome,
    Missing,
    Different,
}

struct Peer {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<Vec<Call>>>,
}

impl Peer {
    fn start(bus: &Bus, scenario: Scenario) -> Self {
        Self::start_with_identity(bus, scenario, Identity::Gnome)
    }

    fn start_with_identity(bus: &Bus, scenario: Scenario, identity: Identity) -> Self {
        let (stop, stopped) = oneshot::channel();
        let (ready, response) = mpsc::sync_channel(1);
        let address = bus.address();
        let peer = thread::spawn(move || {
            zbus::block_on(async {
                let connection = Builder::address(address.clone())
                    .build()
                    .await
                    .expect("fake bus client");
                let mut messages = MessageStream::from(&connection);
                connection
                    .request_name(SERVICE_NAME)
                    .await
                    .expect("own service name");
                let mut other_owner = match identity {
                    Identity::Gnome => {
                        connection.request_name(GNOME_NAME).await.unwrap();
                        None
                    }
                    Identity::Missing => None,
                    Identity::Different => {
                        let other = Builder::address(address.clone()).build().await.unwrap();
                        other.request_name(GNOME_NAME).await.unwrap();
                        Some(other)
                    }
                };
                ready.send(()).unwrap();
                let mut calls = Vec::new();
                let mut key = None;
                let mut stopped = pin!(stopped);
                while let Either::Left((Some(Ok(message)), _)) =
                    select(messages.next(), stopped.as_mut()).await
                {
                    if message.header().message_type() != Type::MethodCall {
                        continue;
                    }
                    let call = inspect_call(&message);
                    calls.push(call);
                    if scenario == Scenario::Silent {
                        let _ = stopped.await;
                        break;
                    }
                    if scenario == Scenario::Disconnect {
                        connection.close().await.expect("close peer mid-call");
                        break;
                    }
                    if call == Call::Session {
                        let moved_name = match scenario {
                            Scenario::AliasMovesAfterSession => Some(GNOME_NAME),
                            Scenario::ServiceMovesAfterSession => Some(SERVICE_NAME),
                            _ => None,
                        };
                        if let Some(moved_name) = moved_name {
                            connection.release_name(moved_name).await.unwrap();
                            let replacement =
                                Builder::address(address.clone()).build().await.unwrap();
                            replacement.request_name(moved_name).await.unwrap();
                            other_owner = Some(replacement);
                        }
                    }
                    serve(&connection, &message, call, scenario, &mut key, &calls).await;
                }
                drop(other_owner);
                calls
            })
        });
        let result = Self {
            stop: Some(stop),
            thread: Some(peer),
        };
        response.recv_timeout(GUARD).expect("fake service ready");
        result
    }

    fn finish(mut self) -> Vec<Call> {
        let _ = self.stop.take().unwrap().send(());
        self.thread
            .take()
            .unwrap()
            .join()
            .expect("scripted peer completed")
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

pub(in crate::credentials::adapters) fn inspect_call(message: &Message) -> Call {
    assert_flags(message);
    let header = message.header();
    match (
        header.interface().map(|i| i.as_str()),
        header.member().map(|m| m.as_str()),
    ) {
        (Some(SERVICE), Some("ReadAlias")) => Call::Alias,
        (Some(SERVICE), Some("SearchItems")) => Call::Search,
        (Some(SERVICE), Some("OpenSession")) => Call::Session,
        (Some(ITEM), Some("GetSecret")) => Call::Secret,
        (Some(SESSION_INTERFACE), Some("Close")) => Call::Close,
        (Some("org.freedesktop.Secret.Collection"), Some("CreateItem")) => Call::Create,
        (Some(ITEM), Some("SetSecret")) => Call::SetSecret,
        (Some(ITEM), Some("Delete")) => Call::Delete,
        (Some("org.freedesktop.Secret.Prompt"), Some("Dismiss")) => Call::Dismiss,
        (Some(PROPERTIES), Some("Get")) => {
            let (interface, property): (String, String) = message.body().deserialize().unwrap();
            match (interface.as_str(), property.as_str()) {
                (ITEM, "Attributes") => Call::Attributes,
                (ITEM, "Locked") => Call::Locked,
                ("org.freedesktop.Secret.Collection", "Locked") => Call::CollectionLocked,
                _ => panic!("unexpected scripted property"),
            }
        }
        // This fails on Unlock, Prompt, or any unexpected message.
        _ => panic!("unexpected method received by Secret Service"),
    }
}

pub(in crate::credentials::adapters) fn assert_flags(message: &Message) {
    let flags = message.header().primary().flags();
    assert!(flags.contains(Flags::NO_AUTO_START));
    assert!(!flags.contains(Flags::ALLOW_INTERACTIVE_AUTH));
}

#[derive(Default)]
pub(in crate::credentials::adapters) struct ScriptSession(Option<super::super::crypto::SessionKey>);

impl ScriptSession {
    pub(in crate::credentials::adapters) async fn respond(
        &mut self,
        connection: &Connection,
        message: &Message,
        call: Call,
        scenario: Scenario,
    ) {
        serve(connection, message, call, scenario, &mut self.0, &[]).await;
    }
}

async fn serve(
    connection: &Connection,
    message: &Message,
    call: Call,
    scenario: Scenario,
    key: &mut Option<super::super::crypto::SessionKey>,
    calls: &[Call],
) {
    if matches!(
        (scenario, call),
        (Scenario::SearchObjectMissing, Call::Search)
            | (Scenario::SessionServiceObjectMissing, Call::Session)
            | (Scenario::LockedItemMissing, Call::Attributes)
            | (Scenario::UnlockedItemMissing, Call::Secret)
    ) {
        return reply_error(
            connection,
            message,
            "org.freedesktop.DBus.Error.UnknownObject",
        )
        .await;
    }
    let header = message.header();
    let path = |value: &'static str| OwnedObjectPath::try_from(value).unwrap();
    let replied = match call {
        Call::Alias => {
            let alias: String = message.body().deserialize().unwrap();
            assert!(alias == "default");
            connection.reply(&header, &path(COLLECTION)).await
        }
        Call::Search => {
            let attributes: Attributes = message.body().deserialize().unwrap();
            assert!(attributes == wanted());
            match scenario {
                Scenario::Denied => {
                    return reply_error(
                        connection,
                        message,
                        "org.freedesktop.DBus.Error.AccessDenied",
                    )
                    .await;
                }
                Scenario::UnknownError => {
                    return reply_error(connection, message, "org.example.Unknown").await;
                }
                _ => {}
            }
            let (unlocked, locked) = match scenario {
                Scenario::Absent => (vec![], vec![]),
                Scenario::Locked | Scenario::LockedItemMissing => (vec![], vec![path(ITEM_PATH)]),
                Scenario::Duplicate => (vec![path(ITEM_PATH)], vec![path(OTHER_ITEM)]),
                _ => (vec![path(ITEM_PATH)], vec![]),
            };
            connection.reply(&header, &(unlocked, locked)).await
        }
        Call::Attributes => {
            let mut actual = wanted();
            if scenario == Scenario::Superset
                || (scenario == Scenario::AttributesChanged && calls.contains(&Call::Secret))
            {
                actual.insert("extra".to_owned(), "a different record".to_owned());
            }
            let property = Variant::from(
                actual
                    .into_iter()
                    .collect::<std::collections::HashMap<_, _>>(),
            );
            connection.reply(&header, &property).await
        }
        Call::Locked => {
            connection
                .reply(
                    &header,
                    &Variant::from(scenario == Scenario::RelockBeforeSession),
                )
                .await
        }
        Call::Session => {
            let (algorithm, client): (String, OwnedVariant) = message.body().deserialize().unwrap();
            assert!(algorithm == ALGORITHM);
            if scenario == Scenario::PlainOnly {
                return reply_error(
                    connection,
                    message,
                    "org.freedesktop.DBus.Error.NotSupported",
                )
                .await;
            }
            if scenario == Scenario::PlainReply {
                return connection
                    .reply(&header, &(Variant::from(""), path(SESSION)))
                    .await
                    .unwrap();
            }
            let client = Vec::<u8>::try_from(client).expect("DH public key bytes");
            assert_eq!(client.len(), 128);
            let mut private = Zeroizing::new([0; 32]);
            private[31] = 2;
            let exchange = Exchange::new(&private).unwrap();
            // Send a minimal-length public key, as deployed peers can do.
            let public = Variant::from(vec![4_u8]);
            *key = Some(exchange.finish(&client).expect("valid DH public key"));
            connection.reply(&header, &(public, path(SESSION))).await
        }
        Call::Secret => {
            let session: OwnedObjectPath = message.body().deserialize().unwrap();
            assert!(session == path(SESSION));
            if scenario == Scenario::SilentAtGet {
                return;
            }
            if scenario == Scenario::RelockAtGet {
                return reply_error(connection, message, "org.freedesktop.Secret.Error.IsLocked")
                    .await;
            }
            let iv = [7; 16];
            let value = key
                .as_ref()
                .expect("negotiated key")
                .encrypt_for_test(&iv, SECRET.as_bytes());
            let reply_session = if scenario == Scenario::WrongSession {
                path(OTHER_ITEM)
            } else {
                session
            };
            connection
                .reply(
                    &header,
                    &((reply_session, iv.to_vec(), value, "text/plain"),),
                )
                .await
        }
        Call::Close => {
            assert_eq!(header.path().map(|path| path.as_str()), Some(SESSION));
            if scenario == Scenario::CloseFails {
                return reply_error(connection, message, "org.freedesktop.DBus.Error.Failed").await;
            }
            connection.reply(&header, &()).await
        }
        _ => panic!("unexpected mutation call in read test"),
    };
    replied.expect("reply on private test bus");
}

async fn reply_error(connection: &Connection, message: &Message, name: &str) {
    let private_detail = format!("{SECRET} {ATTRIBUTE} {ITEM_PATH}");
    connection
        .reply_error(&message.header(), name, &(private_detail,))
        .await
        .expect("scripted error reply");
}

pub(in crate::credentials::adapters) fn assert_redacted(rendered: &str) {
    for canary in [SECRET, ATTRIBUTE, LOCATOR] {
        assert!(!rendered.contains(canary), "private canary escaped");
    }
}

#[test]
fn scripted_reads_are_closed_flagged_and_redacted() {
    use Call::*;
    use Scenario::*;
    test_log_capture::install();
    let bus = Bus::start();
    let success_calls: &[Call] = &[
        Search,
        Attributes,
        Call::Locked,
        Session,
        Secret,
        Attributes,
        Close,
    ];
    for (scenario, expected, calls) in [
        (Found, Ok(true), success_calls),
        (Absent, Ok(false), &[Search]),
        (Superset, Ok(false), &[Search, Attributes]),
        (
            Scenario::Locked,
            Err(Failure::Locked),
            &[Search, Attributes],
        ),
        (
            Duplicate,
            Err(Failure::AmbiguousMatch),
            &[Search, Attributes, Attributes],
        ),
        (
            RelockBeforeSession,
            Err(Failure::Locked),
            &[Search, Attributes, Call::Locked],
        ),
        (
            RelockAtGet,
            Err(Failure::Locked),
            &[Search, Attributes, Call::Locked, Session, Secret, Close],
        ),
        (
            PlainOnly,
            Err(Failure::Unsupported),
            &[Search, Attributes, Call::Locked, Session],
        ),
        (
            PlainReply,
            Err(Failure::Unsupported),
            &[Search, Attributes, Call::Locked, Session, Close],
        ),
        (
            WrongSession,
            Err(Failure::CorruptRecord),
            &[Search, Attributes, Call::Locked, Session, Secret, Close],
        ),
        (
            AttributesChanged,
            Err(Failure::RevisionConflict),
            success_calls,
        ),
        (CloseFails, Ok(true), success_calls),
        (Disconnect, Err(Failure::Unavailable), &[Search]),
        (Denied, Err(Failure::AccessDenied), &[Search]),
        (UnknownError, Err(Failure::Internal), &[Search]),
    ] {
        let peer = Peer::start(&bus, scenario);
        let capture = test_log_capture::ThreadLogCapture::start();
        let read = read_at(bus.address(), &wanted(), DEADLINE);
        assert_redacted(&format!("{read:?}"));
        if let Err(error) = &read {
            assert_redacted(&format!("{error}"));
        }
        assert_eq!(
            read.map(|value| {
                value.is_some_and(|secret| {
                    assert!(
                        secret.into_bytes().as_slice() == SECRET.as_bytes(),
                        "decrypted bytes differ"
                    );
                    true
                })
            })
            .map_err(|error| error.0),
            expected
        );
        assert_eq!(peer.finish(), calls);
        assert_redacted(&test_log_capture::render(&capture.finish()));
    }
    test_log_capture::assert_process_logs_exclude(&[SECRET, ATTRIBUTE, LOCATOR]);
    assert!(!bus.root.join("started").exists());
}

#[test]
fn missing_search_object_is_unavailable() {
    assert_missing_outcome(
        Scenario::SearchObjectMissing,
        Err(Failure::Unavailable),
        &[Call::Search],
    );
}

#[test]
fn missing_open_session_object_is_unavailable() {
    assert_missing_outcome(
        Scenario::SessionServiceObjectMissing,
        Err(Failure::Unavailable),
        &[Call::Search, Call::Attributes, Call::Locked, Call::Session],
    );
}

#[test]
fn missing_locked_item_attributes_leave_the_item_locked() {
    assert_missing_outcome(
        Scenario::LockedItemMissing,
        Err(Failure::Locked),
        &[Call::Search, Call::Attributes],
    );
}

#[test]
fn missing_unlocked_item_at_get_secret_is_absent() {
    assert_missing_outcome(
        Scenario::UnlockedItemMissing,
        Ok(false),
        &[
            Call::Search,
            Call::Attributes,
            Call::Locked,
            Call::Session,
            Call::Secret,
            Call::Close,
        ],
    );
}

fn assert_missing_outcome(scenario: Scenario, expected: Result<bool, Failure>, calls: &[Call]) {
    let bus = Bus::start();
    let peer = Peer::start(&bus, scenario);
    let read = read_at(bus.address(), &wanted(), DEADLINE);
    assert_eq!(peer.finish(), calls);
    assert_eq!(
        read.map(|found| found.is_some()).map_err(|err| err.0),
        expected
    );
}

#[test]
fn ordinary_operations_refuse_missing_or_different_gnome_owner_without_service_calls() {
    use crate::credentials::adapters::native_interaction::NativeInteractionGate;
    use mutations::{Mutation, mutate_at};

    for identity in [Identity::Missing, Identity::Different] {
        let bus = Bus::start();
        let peer = Peer::start_with_identity(&bus, Scenario::Found, identity);
        let attributes = wanted();
        assert_eq!(
            read_at(bus.address(), &attributes, DEADLINE).unwrap_err().0,
            Failure::Unsupported
        );
        for mutation in [
            Mutation::Write(Zeroizing::new(b"fixture secret".to_vec())),
            Mutation::Delete,
        ] {
            let mut lease = NativeInteractionGate::isolated_for_test()
                .acquire()
                .unwrap();
            let (prompt, invocation) = lease.mutation_capabilities();
            assert_eq!(
                mutate_at(
                    bus.address(),
                    prompt,
                    invocation,
                    &attributes,
                    mutation,
                    DEADLINE,
                    None
                ),
                Err(WireError(Failure::Unsupported))
            );
        }
        assert!(peer.finish().is_empty());
    }
}

#[test]
fn a_second_read_rechecks_the_names_after_an_owner_change() {
    let bus = Bus::start();
    let original = Peer::start(&bus, Scenario::Found);
    assert!(
        read_at(bus.address(), &wanted(), DEADLINE)
            .unwrap()
            .is_some()
    );
    assert!(original.finish().contains(&Call::Secret));
    let replacement = Peer::start_with_identity(&bus, Scenario::Found, Identity::Different);
    assert_eq!(
        read_at(bus.address(), &wanted(), DEADLINE).unwrap_err().0,
        Failure::Unsupported
    );
    assert!(replacement.finish().is_empty());
}

#[test]
fn an_owner_change_after_open_session_refuses_get_secret_and_closes_the_original_session() {
    for scenario in [
        Scenario::AliasMovesAfterSession,
        Scenario::ServiceMovesAfterSession,
    ] {
        let bus = Bus::start();
        let peer = Peer::start(&bus, scenario);
        assert_eq!(
            read_at(bus.address(), &wanted(), DEADLINE).unwrap_err().0,
            Failure::Unsupported
        );
        assert_eq!(
            peer.finish(),
            [
                Call::Search,
                Call::Attributes,
                Call::Locked,
                Call::Session,
                Call::Close
            ]
        );
    }
}

#[test]
fn reads_default_alias_explicitly() {
    let bus = Bus::start();
    let peer = Peer::start(&bus, Scenario::Found);
    let collection = read_alias_at(bus.address(), DEADLINE)
        .unwrap()
        .expect("default alias");
    assert!(collection.0.as_str() == COLLECTION);
    assert_eq!(peer.finish(), [Call::Alias]);
}

#[test]
fn unavailable_service_does_not_activate_a_daemon() {
    let bus = Bus::start();
    assert_eq!(
        read_at(bus.address(), &wanted(), DEADLINE).unwrap_err().0,
        Failure::Unavailable
    );
    let marker = bus.root.join("started");
    assert!(!marker.exists(), "read activated a Secret Service daemon");

    // Positive control: this private bus really can run the canary.
    zbus::block_on(async {
        let runtime = zruntime::SharedRuntime::current().unwrap();
        runtime
            .timeout(GUARD, async {
                let bus_client = Builder::address(bus.address()).build().await.unwrap();
                let _ = bus_client
                    .call_method(
                        Some(BUS),
                        BUS_PATH,
                        Some(BUS),
                        "StartServiceByName",
                        &(SERVICE_NAME, 0_u32),
                    )
                    .await;
            })
            .await
            .expect("activation control completed");
    });
    assert!(marker.exists(), "activation canary is inert");
}

#[test]
fn unavailable_bus_is_closed() {
    let bus = Bus::start();
    let address = bus.address();
    drop(bus);
    assert_eq!(
        read_at(address, &wanted(), DEADLINE).unwrap_err().0,
        Failure::Unavailable
    );
}

#[test]
fn unresponsive_peer_hits_deadline() {
    let bus = Bus::start();
    let peer = Peer::start(&bus, Scenario::Silent);
    assert_eq!(
        read_at(bus.address(), &wanted(), Duration::from_secs(2))
            .unwrap_err()
            .0,
        Failure::Unavailable
    );
    assert_eq!(peer.finish(), [Call::Search]);
}

#[test]
fn read_timeout_closes_a_negotiated_session() {
    let bus = Bus::start();
    let peer = Peer::start(&bus, Scenario::SilentAtGet);
    assert_eq!(
        read_at(bus.address(), &wanted(), Duration::from_secs(2))
            .unwrap_err()
            .0,
        Failure::Unavailable
    );
    assert_eq!(
        peer.finish(),
        [
            Call::Search,
            Call::Attributes,
            Call::Locked,
            Call::Session,
            Call::Secret,
            Call::Close
        ]
    );
}

#[test]
fn unresponsive_bus_handshake_hits_deadline() {
    use std::os::unix::net::UnixListener;

    let bus = Bus::start();
    let path = bus.root.join("silent.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let address = format!("unix:path={}", path.display()).parse().unwrap();
    let (accepted, observed) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let peer = thread::spawn(move || {
        let (_socket, _) = listener.accept().unwrap();
        accepted.send(()).unwrap();
        released.recv_timeout(GUARD).unwrap();
    });

    let result = read_at(address, &wanted(), Duration::from_secs(2));
    observed
        .recv_timeout(GUARD)
        .expect("silent bus accepted a connection");
    release.send(()).unwrap();
    peer.join().unwrap();
    assert_eq!(result.unwrap_err().0, Failure::Unavailable);
}

#[test]
fn plain_worker_thread_can_read_while_tokio_is_running() {
    let bus = Bus::start();
    let peer = Peer::start(&bus, Scenario::Found);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let address = bus.address();
    runtime.block_on(async {
        let (done, result) = oneshot::channel();
        let worker = thread::spawn(move || {
            let read = read_at(address, &wanted(), DEADLINE);
            let _ = done.send(read);
        });
        let read = tokio::time::timeout(GUARD, result)
            .await
            .expect("worker deadline")
            .expect("worker read");
        worker.join().unwrap();
        assert!(read.unwrap().is_some());
    });
    assert_eq!(peer.finish().last(), Some(&Call::Close));
}

#[test]
fn runtime_contexts_refuse_blocking_facades_without_sending_messages() {
    let bus = Bus::start();
    let peer = Peer::start(&bus, Scenario::Found);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let read = |context| {
        read_at(bus.address(), &wanted(), DEADLINE)
            .expect_err(context)
            .0
    };
    runtime.block_on(async {
        assert_eq!(read("Tokio task"), Failure::Unsupported);
        let address = bus.address();
        let worker = tokio::task::spawn_blocking(move || read_at(address, &wanted(), DEADLINE));
        let read = tokio::time::timeout(GUARD, worker).await.unwrap().unwrap();
        assert_eq!(read.unwrap_err().0, Failure::Unsupported);
    });
    runtime.shutdown_timeout(GUARD);
    let address = bus.address();
    let code = zbus::block_on(async {
        zruntime::spawn(async move { read_at(address, &wanted(), DEADLINE).unwrap_err().0 })
            .await
            .unwrap()
    });
    assert_eq!(code, Failure::Unsupported);
    let local = zruntime::LocalRuntime::new().unwrap();
    let address = bus.address();
    let task = local.spawn("test runtime guard", async move {
        read_at(address, &wanted(), DEADLINE)
            .expect_err("zruntime local task")
            .0
    });
    assert_eq!(local.block_on(task).unwrap(), Failure::Unsupported);
    assert!(peer.finish().is_empty());
}

mod mutation_tests;

mod qualification_tests;
