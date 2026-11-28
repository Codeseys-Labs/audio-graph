use super::*;
use crate::credentials::adapters::native_interaction::NativeInteractionGate;
use mutations::{DispatchInterruption, Mutation, mutate_at};
use std::collections::HashMap;

const NEW_SECRET: &str = "write canary apricot";
const PROMPT_PATH: &str = "/org/freedesktop/secrets/prompt/locator_canary";
const PROMPT_INTERFACE: &str = "org.freedesktop.Secret.Prompt";
const CALL_DEADLINE: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Initial {
    Absent,
    Present,
    AlreadyDesired,
    DefaultLocked,
    Locked,
    Ambiguous,
    SearchFails,
    ItemAlreadyGone,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behavior {
    Acknowledge,
    CloseFails,
    Prompt,
    PromptNeverSettles,
    PromptChangedState,
    PromptCompleted,
    PromptCompletedPrior,
    LostCommitted,
    LostUnchanged,
    LostReadFails,
    DisconnectAfterCommit,
    DisconnectClient,
    ErrorAfterCommit,
    RelockAtMutation,
    DisappearAtMutation,
    AccessDenied,
    MissingSession,
    PlainSession,
    PrepareReadFails,
    ExpireBeforeSend,
}

#[derive(Debug, PartialEq, Eq)]
struct Step {
    call: Call,
    client: usize,
}

struct Store {
    initial: Initial,
    behavior: Behavior,
    value: Option<Zeroizing<Vec<u8>>>,
    dispatched: bool,
    callers: HashMap<String, usize>,
    sessions: HashMap<usize, super::super::super::crypto::SessionKey>,
    ledger: Vec<Step>,
    disconnect: Option<oneshot::Sender<()>>,
}

impl Store {
    fn new(initial: Initial, behavior: Behavior, disconnect: Option<oneshot::Sender<()>>) -> Self {
        Self {
            initial,
            behavior,
            value: match initial {
                Initial::Absent | Initial::DefaultLocked => None,
                Initial::AlreadyDesired => Some(Zeroizing::new(NEW_SECRET.as_bytes().to_vec())),
                _ => Some(Zeroizing::new(SECRET.as_bytes().to_vec())),
            },
            dispatched: false,
            callers: HashMap::new(),
            sessions: HashMap::new(),
            ledger: Vec::new(),
            disconnect,
        }
    }

    async fn serve(&mut self, connection: &Connection, message: &Message) {
        let header = message.header();
        if header
            .interface()
            .is_some_and(|interface| interface.as_str() == PROMPT_INTERFACE)
        {
            assert!(
                header
                    .member()
                    .is_some_and(|member| member.as_str() == "Dismiss"),
                "a mutation may dismiss a prompt but must never display one"
            );
        }
        let call = inspect_call(message);
        let header = message.header();
        let sender = header.sender().unwrap().as_str();
        let next_client = self.callers.len();
        let client = *self.callers.entry(sender.to_owned()).or_insert(next_client);
        self.ledger.push(Step { call, client });
        let locked = self.initial == Initial::Locked
            || (self.dispatched && self.behavior == Behavior::RelockAtMutation);

        let result = match call {
            Call::Search => {
                let attributes: Attributes = message.body().deserialize().unwrap();
                assert!(attributes == wanted(), "lookup attributes differ");
                if self.initial == Initial::SearchFails
                    || (self.dispatched && self.behavior == Behavior::LostReadFails)
                {
                    return reply_error(connection, message, "org.freedesktop.DBus.Error.Failed")
                        .await;
                }
                let (unlocked, locked) = if self.value.is_none() {
                    (vec![], vec![])
                } else if locked {
                    (vec![], vec![path(ITEM_PATH)])
                } else if self.initial == Initial::Ambiguous {
                    (vec![path(ITEM_PATH), path(OTHER_ITEM)], vec![])
                } else {
                    (vec![path(ITEM_PATH)], vec![])
                };
                connection.reply(&header, &(unlocked, locked)).await
            }
            Call::Attributes => {
                if self.initial == Initial::ItemAlreadyGone {
                    return reply_error(
                        connection,
                        message,
                        "org.freedesktop.DBus.Error.UnknownObject",
                    )
                    .await;
                }
                let value = Variant::from(wanted().into_iter().collect::<HashMap<_, _>>());
                connection.reply(&header, &value).await
            }
            Call::Alias => {
                let name: String = message.body().deserialize().unwrap();
                assert!(name == "default");
                connection.reply(&header, &path(COLLECTION)).await
            }
            Call::CollectionLocked => {
                assert!(header.path().is_some_and(|p| p.as_str() == COLLECTION));
                connection
                    .reply(
                        &header,
                        &Variant::from(self.initial == Initial::DefaultLocked),
                    )
                    .await
            }
            Call::Locked => connection.reply(&header, &Variant::from(locked)).await,
            Call::Session => {
                let (algorithm, public): (String, OwnedVariant) =
                    message.body().deserialize().unwrap();
                assert!(algorithm == ALGORITHM);
                let public: Vec<u8> = public.try_into().unwrap();
                assert_eq!(public.len(), 128);
                let mut private = Zeroizing::new([0; 32]);
                private[31] = 2;
                let exchange = Exchange::new(&private).unwrap();
                assert!(
                    self.sessions
                        .insert(client, exchange.finish(&public).unwrap())
                        .is_none()
                );
                let output = if self.behavior == Behavior::PlainSession {
                    Variant::from("")
                } else {
                    Variant::from(vec![4_u8])
                };
                connection.reply(&header, &(output, path(SESSION))).await
            }
            Call::Secret => {
                if self.behavior == Behavior::PrepareReadFails {
                    return reply_error(connection, message, "org.freedesktop.DBus.Error.Failed")
                        .await;
                }
                let session: OwnedObjectPath = message.body().deserialize().unwrap();
                assert!(session == path(SESSION));
                let iv = [7; 16];
                let encrypted = self.sessions[&client]
                    .encrypt_for_test(&iv, self.value.as_ref().unwrap().as_slice());
                connection
                    .reply(
                        &header,
                        &((session, iv.to_vec(), encrypted, "application/octet-stream"),),
                    )
                    .await
            }
            Call::Close => {
                assert!(header.path().is_some_and(|p| p.as_str() == SESSION));
                assert!(
                    self.sessions.remove(&client).is_some(),
                    "close must target an open session"
                );
                if self.behavior == Behavior::CloseFails {
                    return reply_error(connection, message, "org.freedesktop.DBus.Error.Failed")
                        .await;
                }
                connection.reply(&header, &()).await
            }
            Call::Create | Call::SetSecret | Call::Delete => {
                self.apply(connection, message, call, client).await;
                return;
            }
            Call::Dismiss => {
                assert!(header.path().is_some_and(|p| p.as_str() == PROMPT_PATH));
                assert!(matches!(
                    self.behavior,
                    Behavior::Prompt
                        | Behavior::PromptNeverSettles
                        | Behavior::PromptChangedState
                        | Behavior::PromptCompleted
                        | Behavior::PromptCompletedPrior
                ));
                if self.behavior != Behavior::PromptNeverSettles {
                    // Emit before replying. A client that subscribes after
                    // Dismiss will miss the only completion signal.
                    connection
                        .emit_signal(
                            Some(sender),
                            PROMPT_PATH,
                            PROMPT_INTERFACE,
                            "Completed",
                            &(
                                !matches!(
                                    self.behavior,
                                    Behavior::PromptCompleted | Behavior::PromptCompletedPrior
                                ),
                                Variant::from("/"),
                            ),
                        )
                        .await
                        .unwrap();
                }
                connection.reply(&header, &()).await
            }
        };
        result.expect("scripted service reply");
    }

    async fn apply(
        &mut self,
        connection: &Connection,
        message: &Message,
        call: Call,
        client: usize,
    ) {
        assert!(!self.dispatched, "mutation dispatched more than once");
        assert_eq!(client, 0);
        self.dispatched = true;
        let header = message.header();
        let new_value = match call {
            Call::Create => {
                assert!(header.path().is_some_and(|p| p.as_str() == COLLECTION));
                let (mut properties, secret, replace): (
                    HashMap<String, OwnedVariant>,
                    SecretReply,
                    bool,
                ) = message.body().deserialize().unwrap();
                assert!(!replace, "CreateItem must not select a replacement");
                assert_eq!(properties.len(), 2);
                let attributes: Attributes = properties
                    .remove("org.freedesktop.Secret.Item.Attributes")
                    .unwrap()
                    .try_into()
                    .unwrap();
                assert!(attributes == wanted(), "created attributes differ");
                let label: String = properties
                    .remove("org.freedesktop.Secret.Item.Label")
                    .unwrap()
                    .try_into()
                    .unwrap();
                assert!(label == "AudioGraph credential");
                Some(self.verify_write(client, secret))
            }
            Call::SetSecret => {
                assert!(header.path().is_some_and(|p| p.as_str() == ITEM_PATH));
                let (secret,): (SecretReply,) = message.body().deserialize().unwrap();
                Some(self.verify_write(client, secret))
            }
            Call::Delete => {
                assert!(header.path().is_some_and(|p| p.as_str() == ITEM_PATH));
                None
            }
            _ => panic!("invalid mutation script"),
        };
        match self.behavior {
            Behavior::RelockAtMutation => {
                reply_error(connection, message, "org.freedesktop.Secret.Error.IsLocked").await;
                return;
            }
            Behavior::DisappearAtMutation | Behavior::MissingSession => {
                if self.behavior == Behavior::DisappearAtMutation {
                    self.value = None;
                }
                reply_error(
                    connection,
                    message,
                    "org.freedesktop.DBus.Error.UnknownObject",
                )
                .await;
                return;
            }
            Behavior::AccessDenied => {
                reply_error(
                    connection,
                    message,
                    "org.freedesktop.DBus.Error.AccessDenied",
                )
                .await;
                return;
            }
            Behavior::Prompt
            | Behavior::PromptNeverSettles
            | Behavior::PromptChangedState
            | Behavior::PromptCompleted
            | Behavior::PromptCompletedPrior => {
                if matches!(
                    self.behavior,
                    Behavior::PromptChangedState | Behavior::PromptCompleted
                ) {
                    self.value = new_value;
                }
                match call {
                    Call::Create => connection
                        .reply(&header, &(path("/"), path(PROMPT_PATH)))
                        .await
                        .unwrap(),
                    Call::Delete => connection.reply(&header, &path(PROMPT_PATH)).await.unwrap(),
                    _ => panic!("SetSecret cannot return a prompt"),
                }
                return;
            }
            Behavior::LostUnchanged => return,
            _ => self.value = new_value,
        }
        match self.behavior {
            Behavior::LostCommitted | Behavior::LostReadFails => return,
            Behavior::DisconnectAfterCommit => {
                // The bus can no longer route Close after this simulated
                // owner disconnect. The peer's sessions disappear with it.
                self.sessions.clear();
                connection.clone().close().await.unwrap();
                return;
            }
            Behavior::DisconnectClient => {
                self.sessions.remove(&client);
                self.disconnect.take().unwrap().send(()).unwrap();
                return;
            }
            Behavior::ErrorAfterCommit => {
                reply_error(connection, message, "org.freedesktop.DBus.Error.Failed").await;
                return;
            }
            _ => {}
        }
        match call {
            Call::Create => connection
                .reply(&header, &(path(ITEM_PATH), path("/")))
                .await
                .unwrap(),
            Call::Delete => connection.reply(&header, &path("/")).await.unwrap(),
            Call::SetSecret => connection.reply(&header, &()).await.unwrap(),
            _ => panic!("invalid mutation reply"),
        }
    }

    fn verify_write(&self, client: usize, secret: SecretReply) -> Zeroizing<Vec<u8>> {
        let (session, iv, ciphertext, content_type) = secret;
        assert!(session == path(SESSION));
        assert_eq!(iv.len(), 16);
        assert!(content_type == "application/octet-stream");
        let actual = self.sessions[&client].decrypt(&iv, ciphertext).unwrap();
        assert!(
            actual.as_slice() == NEW_SECRET.as_bytes(),
            "wire IV did not decrypt the expected secret"
        );
        actual
    }
}

#[test]
fn mutations_never_display_prompts() {
    for (initial, behavior, mutation, expected, dismissals) in [
        (
            Initial::Absent,
            Behavior::Prompt,
            put(),
            MutationOutcome::InteractionRequired,
            1,
        ),
        (
            Initial::Present,
            Behavior::Acknowledge,
            put(),
            MutationOutcome::Committed,
            0,
        ),
        (
            Initial::Present,
            Behavior::Prompt,
            Mutation::Delete,
            MutationOutcome::InteractionRequired,
            1,
        ),
    ] {
        let calls = run(initial, behavior, mutation, Ok(expected));
        assert_eq!(
            calls
                .iter()
                .filter(|step| step.call == Call::Dismiss)
                .count(),
            dismissals
        );
        assert_eq!(mutation_calls(&calls).len(), 1);
    }
}

fn path(value: &'static str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(value).unwrap()
}

fn run(
    initial: Initial,
    behavior: Behavior,
    mutation: Mutation,
    expected: WireResult<MutationOutcome>,
) -> Vec<Step> {
    let capture = test_log_capture::ThreadLogCapture::start();
    let bus = Bus::start();
    let (stop, stopped) = oneshot::channel();
    let (ready, response) = mpsc::sync_channel(1);
    let (disconnect, interruption) = match behavior {
        Behavior::DisconnectClient => {
            let (send, receive) = oneshot::channel();
            (Some(send), Some(DispatchInterruption::Disconnect(receive)))
        }
        Behavior::ExpireBeforeSend => (None, Some(DispatchInterruption::ExpireBeforeSend)),
        _ => (None, None),
    };
    let address = bus.address();
    let peer = thread::spawn(move || {
        zbus::block_on(async {
            let connection = Builder::address(address).build().await.unwrap();
            let mut messages = MessageStream::from(&connection);
            connection.request_name(SERVICE_NAME).await.unwrap();
            connection.request_name(GNOME_NAME).await.unwrap();
            ready.send(()).unwrap();
            let mut store = Store::new(initial, behavior, disconnect);
            let mut stopped = pin!(stopped);
            while let Either::Left((Some(Ok(message)), _)) =
                select(messages.next(), stopped.as_mut()).await
            {
                if message.header().message_type() == Type::MethodCall {
                    store.serve(&connection, &message).await;
                }
            }
            assert!(
                store.sessions.is_empty(),
                "client left a negotiated session open"
            );
            store.ledger
        })
    });
    response.recv_timeout(GUARD).expect("mutating peer ready");
    let gate = NativeInteractionGate::isolated_for_test();
    let mut lease = gate.acquire().unwrap();
    let (prompt, invocation) = lease.mutation_capabilities();
    if behavior == Behavior::ExpireBeforeSend {
        // An earlier write on this same lease already set the aggregate flag.
        invocation.mark_started();
    }
    let outcome = mutate_at(
        bus.address(),
        prompt,
        invocation,
        &wanted(),
        mutation,
        CALL_DEADLINE,
        interruption,
    );
    let rendered = format!("{outcome:?}");
    assert_redacted(&rendered);
    assert!(!rendered.contains(NEW_SECRET));
    if let Err(error) = &outcome {
        let rendered = error.to_string();
        assert_redacted(&rendered);
        assert!(!rendered.contains(NEW_SECRET));
    }
    if expected == Err(Failure::CommitUnknown.into()) {
        assert_eq!(
            mutate_at(
                bus.address(),
                prompt,
                invocation,
                &wanted(),
                Mutation::Delete,
                CALL_DEADLINE,
                None,
            ),
            Err(Failure::StalledWorker.into())
        );
    }
    drop(lease);
    if behavior == Behavior::ExpireBeforeSend {
        assert!(
            gate.acquire().is_ok(),
            "pre-send expiration must leave the gate healthy"
        );
    }
    let _ = stop.send(());
    let calls = peer.join().unwrap();
    assert_eq!(outcome, expected);
    let logs = test_log_capture::render(&capture.finish());
    assert_redacted(&logs);
    assert!(!logs.contains(NEW_SECRET));
    test_log_capture::assert_process_logs_exclude(&[SECRET, NEW_SECRET, ATTRIBUTE, LOCATOR]);
    assert!(calls.first().is_some_and(|step| step.call == Call::Search));
    assert!(!bus.root.join("started").exists());
    calls
}

fn put() -> Mutation {
    Mutation::Write(Zeroizing::new(NEW_SECRET.as_bytes().to_vec()))
}

fn mutation_calls(calls: &[Step]) -> Vec<Call> {
    calls
        .iter()
        .map(|step| step.call)
        .filter(|call| matches!(call, Call::Create | Call::SetSecret | Call::Delete))
        .collect()
}

fn assert_readback(calls: &[Step]) {
    assert!(
        calls
            .iter()
            .any(|step| step.client == 1 && step.call == Call::Search)
    );
    for client in 0..=1 {
        let count = |call| {
            calls
                .iter()
                .filter(|step| step.client == client && step.call == call)
                .count()
        };
        assert_eq!(count(Call::Session), count(Call::Close));
    }
}

#[test]
fn writes_absent_and_replaces_the_exact_existing_item() {
    for (initial, expected) in [
        (Initial::Absent, Call::Create),
        (Initial::Present, Call::SetSecret),
    ] {
        let calls = run(
            initial,
            Behavior::Acknowledge,
            put(),
            Ok(MutationOutcome::Committed),
        );
        assert_eq!(mutation_calls(&calls), [expected]);
        assert_readback(&calls);
    }
}

#[test]
fn refuses_uncertain_or_non_unique_searches_before_mutating() {
    for (initial, expected) in [
        (Initial::Locked, Failure::Locked),
        (Initial::Ambiguous, Failure::AmbiguousMatch),
        (Initial::SearchFails, Failure::Internal),
        (Initial::ItemAlreadyGone, Failure::Missing),
    ] {
        for mutation in [put(), Mutation::Delete] {
            let calls = run(
                initial,
                Behavior::Acknowledge,
                mutation,
                Err(expected.into()),
            );
            assert!(mutation_calls(&calls).is_empty());
            assert!(calls.iter().all(|step| step.client == 0));
        }
    }
    let calls = run(
        Initial::DefaultLocked,
        Behavior::Acknowledge,
        put(),
        Err(Failure::Locked.into()),
    );
    assert!(mutation_calls(&calls).is_empty());
}

#[test]
fn deletes_one_exact_item_and_treats_definite_absence_as_a_noop() {
    let calls = run(
        Initial::Present,
        Behavior::Acknowledge,
        Mutation::Delete,
        Ok(MutationOutcome::Committed),
    );
    assert_eq!(mutation_calls(&calls), [Call::Delete]);
    assert_readback(&calls);
    assert!(
        calls
            .iter()
            .all(|step| !matches!(step.call, Call::Session | Call::Secret))
    );
    let calls = run(
        Initial::Absent,
        Behavior::Acknowledge,
        Mutation::Delete,
        Ok(MutationOutcome::Committed),
    );
    assert_eq!(
        calls,
        [Step {
            call: Call::Search,
            client: 0
        }]
    );
}

#[test]
fn dismisses_create_and_delete_prompts_and_verifies_the_prior_state() {
    for (initial, mutation, call) in [
        (Initial::Absent, put(), Call::Create),
        (Initial::Present, Mutation::Delete, Call::Delete),
    ] {
        let calls = run(
            initial,
            Behavior::Prompt,
            mutation,
            Ok(MutationOutcome::InteractionRequired),
        );
        assert_eq!(mutation_calls(&calls), [call]);
        assert_readback(&calls);
        let sent: Vec<_> = calls
            .iter()
            .filter(|step| step.client == 0)
            .map(|step| step.call)
            .collect();
        if call == Call::Create {
            assert!(sent.ends_with(&[call, Call::Dismiss, Call::Close]));
        } else {
            assert!(sent.ends_with(&[call, Call::Dismiss]));
        }
    }
}

#[test]
fn latches_an_unsettled_prompt_or_one_with_changed_state() {
    for behavior in [Behavior::PromptNeverSettles, Behavior::PromptChangedState] {
        let calls = run(
            Initial::Absent,
            behavior,
            put(),
            Err(Failure::CommitUnknown.into()),
        );
        assert_eq!(mutation_calls(&calls), [Call::Create]);
        assert!(
            calls
                .iter()
                .any(|step| step.client == 0 && step.call == Call::Dismiss)
        );
        assert_readback(&calls);
    }
}

#[test]
fn reconciles_lost_replies_with_fresh_readback_and_never_retries() {
    for (behavior, expected) in [
        (Behavior::LostCommitted, Ok(MutationOutcome::Committed)),
        (Behavior::LostUnchanged, Err(Failure::CommitUnknown.into())),
        (Behavior::LostReadFails, Err(Failure::CommitUnknown.into())),
        (Behavior::ErrorAfterCommit, Ok(MutationOutcome::Committed)),
    ] {
        let calls = run(Initial::Present, behavior, put(), expected);
        assert_eq!(mutation_calls(&calls), [Call::SetSecret]);
        assert_readback(&calls);
    }
    let calls = run(
        Initial::Present,
        Behavior::LostCommitted,
        Mutation::Delete,
        Ok(MutationOutcome::Committed),
    );
    assert_eq!(mutation_calls(&calls), [Call::Delete]);
    assert_readback(&calls);

    let calls = run(
        Initial::AlreadyDesired,
        Behavior::LostUnchanged,
        put(),
        Err(Failure::CommitUnknown.into()),
    );
    assert_eq!(mutation_calls(&calls), [Call::SetSecret]);
    assert_readback(&calls);
}

#[test]
fn a_disconnected_owner_after_dispatch_latches_commit_unknown() {
    let calls = run(
        Initial::Absent,
        Behavior::DisconnectAfterCommit,
        put(),
        Err(Failure::CommitUnknown.into()),
    );
    assert_eq!(mutation_calls(&calls), [Call::Create]);
    assert!(calls.iter().all(|step| step.client == 0));
}

#[test]
fn confirms_terminal_rejections_when_an_item_relocks_or_disappears() {
    for (behavior, code) in [
        (Behavior::RelockAtMutation, Failure::Locked),
        (Behavior::DisappearAtMutation, Failure::Missing),
    ] {
        for (mutation, call) in [(put(), Call::SetSecret), (Mutation::Delete, Call::Delete)] {
            let calls = run(
                Initial::Present,
                behavior,
                mutation,
                Ok(MutationOutcome::NotCommitted(code)),
            );
            assert_eq!(mutation_calls(&calls), [call]);
            assert_readback(&calls);
        }
    }
}

#[test]
fn close_failure_does_not_change_a_verified_commit() {
    let calls = run(
        Initial::Absent,
        Behavior::CloseFails,
        put(),
        Ok(MutationOutcome::Committed),
    );
    assert_eq!(mutation_calls(&calls), [Call::Create]);
    assert_readback(&calls);
}

#[test]
fn lost_create_replies_use_a_live_peer_and_lost_delete_can_leave_the_item() {
    for (behavior, expected) in [
        (Behavior::LostCommitted, Ok(MutationOutcome::Committed)),
        (Behavior::LostUnchanged, Err(Failure::CommitUnknown.into())),
        (Behavior::LostReadFails, Err(Failure::CommitUnknown.into())),
    ] {
        let calls = run(Initial::Absent, behavior, put(), expected);
        assert_eq!(mutation_calls(&calls), [Call::Create]);
        assert_readback(&calls);
    }
    let calls = run(
        Initial::Present,
        Behavior::LostUnchanged,
        Mutation::Delete,
        Err(Failure::CommitUnknown.into()),
    );
    assert_eq!(mutation_calls(&calls), [Call::Delete]);
    assert_readback(&calls);
}

#[test]
fn create_reconciles_collection_relock_or_disappearance_and_access_denied() {
    for (behavior, code) in [
        (Behavior::RelockAtMutation, Failure::Locked),
        (Behavior::DisappearAtMutation, Failure::Unavailable),
        (Behavior::AccessDenied, Failure::AccessDenied),
    ] {
        let calls = run(
            Initial::Absent,
            behavior,
            put(),
            Ok(MutationOutcome::NotCommitted(code)),
        );
        assert_eq!(mutation_calls(&calls), [Call::Create]);
        assert_readback(&calls);
    }
}

#[test]
fn preparation_errors_close_the_session_without_dispatch() {
    for (behavior, code) in [
        (Behavior::PlainSession, Failure::Unsupported),
        (Behavior::PrepareReadFails, Failure::Internal),
        (Behavior::ExpireBeforeSend, Failure::Unavailable),
    ] {
        let calls = run(Initial::Present, behavior, put(), Err(code.into()));
        assert!(mutation_calls(&calls).is_empty());
        assert!(calls.iter().all(|step| step.client == 0));
        assert_eq!(calls.last().unwrap().call, Call::Close);
    }
}

#[test]
fn missing_session_is_unavailable_when_readback_proves_the_item_is_present() {
    let calls = run(
        Initial::Present,
        Behavior::MissingSession,
        put(),
        Ok(MutationOutcome::NotCommitted(Failure::Unavailable)),
    );
    assert_eq!(mutation_calls(&calls), [Call::SetSecret]);
    assert_readback(&calls);
}

#[test]
fn completed_prompt_is_verified_and_disconnect_of_only_the_client_can_commit() {
    let calls = run(
        Initial::Absent,
        Behavior::PromptCompleted,
        put(),
        Ok(MutationOutcome::Committed),
    );
    assert_eq!(mutation_calls(&calls), [Call::Create]);
    assert!(calls.iter().any(|step| step.call == Call::Dismiss));
    assert_readback(&calls);

    let calls = run(
        Initial::Absent,
        Behavior::DisconnectClient,
        put(),
        Ok(MutationOutcome::Committed),
    );
    assert_eq!(mutation_calls(&calls), [Call::Create]);
    assert!(
        !calls
            .iter()
            .any(|step| step.client == 0 && step.call == Call::Close)
    );
    assert!(
        calls
            .iter()
            .any(|step| step.client == 1 && step.call == Call::Close)
    );
}

#[test]
fn non_dismissed_completion_with_prior_state_requires_interaction() {
    let calls = run(
        Initial::Absent,
        Behavior::PromptCompletedPrior,
        put(),
        Ok(MutationOutcome::InteractionRequired),
    );
    assert_eq!(mutation_calls(&calls), [Call::Create]);
    assert!(calls.iter().any(|step| step.call == Call::Dismiss));
    assert_readback(&calls);
}
