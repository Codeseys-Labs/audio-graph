use super::*;
use crate::credentials::adapters::keyring_entry::EntryLocator;
use crate::credentials::adapters::linux_secret_service_wire::{
    locked_target_at, scripted_peer as script,
};
use crate::credentials::adapters::native_interaction::NativeInteractionGate;
use crate::test_log_capture;
use std::sync::{Arc, mpsc};
use std::thread;
use tokio::sync::oneshot;
use zbus::connection::Builder;

const BUS: &str = "org.freedesktop.DBus";
const SERVICE_NAME: &str = "org.freedesktop.secrets";
const PROMPT_PATH: &str = "/org/freedesktop/secrets/prompt/locator_canary";
const WINDOW: &str = "parent window canary";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behavior {
    Accept,
    UserDismisses,
    CancelOnPrompt,
    NeverCompletes,
    IgnoresDismiss,
    NoPrompt,
    StillLocked,
    VerificationFails,
    VerificationDenied,
    ChangedTarget,
    QueuedApproval,
    QueuedDismissal,
    DeadlineQueuedApproval,
    DeadlineQueuedDismissal,
    ApprovesDuringCancel,
    ApprovesDuringDeadline,
    IgnoresDeadline,
    WrongWorker,
    Panic,
    ForgedSender,
    ForgedPath,
    KWallet,
    Stalled,
    CommitUnknown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecoveryScope {
    Item,
    DefaultCollection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Ordinary(script::Call),
    Unlock,
    Prompt,
    Dismiss,
    Verification,
}

fn run(
    behavior: Behavior,
    scope: RecoveryScope,
    window: Option<&str>,
) -> (Result<RecoveryOutcome, Failure>, Vec<Event>) {
    let capture = test_log_capture::ThreadLogCapture::start();
    let bus = script::Bus::start();
    let cancellation = Arc::new(CancellationToken::new());
    let cancellation_for_peer = Arc::clone(&cancellation);
    let (stop, stopped) = oneshot::channel();
    let (ready, response) = mpsc::sync_channel(1);
    let address = bus.address();
    let expected_window = window.unwrap_or_default().to_owned();
    let scripted = thread::spawn(move || {
        zbus::block_on(async {
            let connection = Builder::address(address.clone()).build().await.unwrap();
            let mut messages = MessageStream::from(&connection);
            connection.request_name(SERVICE_NAME).await.unwrap();
            let _other_owner = if behavior == Behavior::KWallet {
                let other = Builder::address(address.clone()).build().await.unwrap();
                other.request_name("org.gnome.keyring").await.unwrap();
                Some(other)
            } else {
                connection.request_name("org.gnome.keyring").await.unwrap();
                None
            };
            ready.send(()).unwrap();
            let mut retired = false;
            let mut shown = false;
            let mut unlocked = false;
            let mut interactive = None::<String>;
            let mut original_client = None::<String>;
            let mut events = Vec::new();
            let mut session = script::ScriptSession::default();
            let mut stopped = pin!(stopped);
            while let Either::Left((Some(Ok(message)), _)) =
                select(messages.next(), stopped.as_mut()).await
            {
                let header = message.header();
                if header.message_type() != Type::MethodCall {
                    continue;
                }
                script::assert_flags(&message);
                let sender = header.sender().unwrap().as_str();
                let probe = original_client.get_or_insert_with(|| sender.to_owned());
                let path = header.path().unwrap().as_str();
                let event = match (
                    header.interface().map(|name| name.as_str()),
                    header.member().map(|name| name.as_str()),
                ) {
                    (Some(SERVICE), Some(UNLOCK_METHOD)) => {
                        assert_eq!(path, SERVICE_PATH);
                        assert_ne!(sender, probe);
                        assert!(interactive.replace(sender.to_owned()).is_none());
                        let objects: Vec<OwnedObjectPath> = message.body().deserialize().unwrap();
                        let expected = match scope {
                            RecoveryScope::Item => script::ITEM_PATH,
                            RecoveryScope::DefaultCollection => script::COLLECTION,
                        };
                        assert!(objects.len() == 1 && objects[0].as_str() == expected);
                        let mut result = Vec::new();
                        let prompt = if behavior == Behavior::NoPrompt {
                            unlocked = true;
                            result = objects;
                            "/"
                        } else {
                            PROMPT_PATH
                        };
                        connection
                            .reply(
                                &header,
                                &(result, OwnedObjectPath::try_from(prompt).unwrap()),
                            )
                            .await
                            .unwrap();
                        Event::Unlock
                    }
                    (Some(PROMPT), Some(PROMPT_METHOD)) => {
                        assert_eq!(path, PROMPT_PATH);
                        assert_eq!(interactive.as_deref(), Some(sender));
                        assert!(!shown);
                        let window: String = message.body().deserialize().unwrap();
                        assert!(window == expected_window);
                        shown = true;
                        if behavior == Behavior::ForgedSender {
                            let other = Builder::address(address.clone()).build().await.unwrap();
                            emit_completion_at(&other, Some(sender), PROMPT_PATH, false).await;
                            // A bus-driver round trip ensures the forged signal was sent
                            // before the genuine completion.
                            other
                                .call_method(
                                    Some(BUS),
                                    "/org/freedesktop/DBus",
                                    Some(BUS),
                                    "GetId",
                                    &(),
                                )
                                .await
                                .unwrap();
                        }
                        if behavior == Behavior::ForgedPath {
                            emit_completion_at(&connection, Some(sender), "/other_prompt", false)
                                .await;
                        }
                        if matches!(
                            behavior,
                            Behavior::Accept
                                | Behavior::UserDismisses
                                | Behavior::StillLocked
                                | Behavior::VerificationFails
                                | Behavior::VerificationDenied
                                | Behavior::QueuedApproval
                                | Behavior::QueuedDismissal
                                | Behavior::DeadlineQueuedApproval
                                | Behavior::DeadlineQueuedDismissal
                                | Behavior::ForgedSender
                                | Behavior::ForgedPath
                        ) {
                            unlocked = matches!(
                                behavior,
                                Behavior::Accept
                                    | Behavior::VerificationFails
                                    | Behavior::VerificationDenied
                                    | Behavior::QueuedApproval
                                    | Behavior::DeadlineQueuedApproval
                            );
                            // Broadcast and emit before replying. The private bus
                            // only routes this signal if AddMatch preceded Prompt.
                            emit_completion(
                                &connection,
                                matches!(
                                    behavior,
                                    Behavior::UserDismisses
                                        | Behavior::QueuedDismissal
                                        | Behavior::DeadlineQueuedDismissal
                                        | Behavior::ForgedSender
                                        | Behavior::ForgedPath,
                                ),
                            )
                            .await;
                            shown = false;
                        }
                        connection.reply(&header, &()).await.unwrap();
                        if matches!(
                            behavior,
                            Behavior::CancelOnPrompt
                                | Behavior::IgnoresDismiss
                                | Behavior::ApprovesDuringCancel
                        ) {
                            cancellation_for_peer.cancel();
                        }
                        Event::Prompt
                    }
                    (Some(PROMPT), Some("Dismiss")) => {
                        assert_eq!(path, PROMPT_PATH);
                        assert_eq!(interactive.as_deref(), Some(sender));
                        assert!(shown);
                        if !matches!(
                            behavior,
                            Behavior::IgnoresDismiss | Behavior::IgnoresDeadline
                        ) {
                            unlocked = matches!(
                                behavior,
                                Behavior::ApprovesDuringCancel | Behavior::ApprovesDuringDeadline
                            );
                            emit_completion(&connection, !unlocked).await;
                            shown = false;
                        }
                        connection.reply(&header, &()).await.unwrap();
                        Event::Dismiss
                    }
                    _ => {
                        if let Some(interactive) = &interactive
                            && !retired
                        {
                            assert_ne!(interactive, sender);
                            let reply = connection
                                .call_method(
                                    Some(BUS),
                                    "/org/freedesktop/DBus",
                                    Some(BUS),
                                    "NameHasOwner",
                                    &(interactive.as_str(),),
                                )
                                .await
                                .unwrap();
                            assert!(!reply.body().deserialize::<bool>().unwrap());
                            retired = true;
                            events.push(Event::Verification);
                        }
                        let call = script::inspect_call(&message);
                        if call == script::Call::CollectionLocked {
                            connection
                                .reply(&header, &zbus::Variant::from(!unlocked))
                                .await
                                .unwrap();
                        } else {
                            let scenario = if behavior == Behavior::ChangedTarget && sender != probe
                            {
                                script::Scenario::Duplicate
                            } else if retired && behavior == Behavior::VerificationFails {
                                script::Scenario::RelockAtGet
                            } else if retired && behavior == Behavior::VerificationDenied {
                                script::Scenario::Denied
                            } else if scope == RecoveryScope::DefaultCollection {
                                script::Scenario::Absent
                            } else if unlocked {
                                script::Scenario::Found
                            } else {
                                script::Scenario::Locked
                            };
                            session.respond(&connection, &message, call, scenario).await;
                        }
                        Event::Ordinary(call)
                    }
                };
                events.push(event);
            }
            assert_eq!(
                shown,
                matches!(
                    behavior,
                    Behavior::IgnoresDismiss | Behavior::IgnoresDeadline
                )
            );
            events
        })
    });
    response.recv_timeout(script::GUARD).unwrap();
    let gate = NativeInteractionGate::isolated_for_test();
    let target = {
        let lease = gate.acquire().unwrap();
        let target = locked_target_at(
            bus.address(),
            lease.forbid_prompt(),
            &EntryLocator::authority(),
        )
        .unwrap()
        .expect("an exact locked target");
        if behavior == Behavior::Stalled {
            assert_eq!(
                crate::credentials::adapters::native_interaction::latch_forbid_uncertainty(
                    lease.forbid_prompt()
                ),
                Failure::StalledWorker
            );
        }
        target
    };
    if behavior == Behavior::CommitUnknown {
        let mut lease = gate.acquire().unwrap();
        assert_eq!(
            lease.mutation_capabilities().1.latch_commit_unknown(),
            Failure::CommitUnknown
        );
    }
    let budget = if matches!(
        behavior,
        Behavior::NeverCompletes
            | Behavior::ApprovesDuringDeadline
            | Behavior::DeadlineQueuedApproval
            | Behavior::DeadlineQueuedDismissal
            | Behavior::IgnoresDeadline
    ) {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(4)
    };
    let facade = recovery_facade(
        target,
        window.map(str::to_owned),
        budget,
        TestOptions {
            gate: Some(gate),
            address: Some(bus.address()),
            cancel_with_queued_completion: matches!(
                behavior,
                Behavior::QueuedApproval | Behavior::QueuedDismissal
            ),
            timeout_with_queued_completion: matches!(
                behavior,
                Behavior::DeadlineQueuedApproval | Behavior::DeadlineQueuedDismissal
            ),
            panic_after_prompt: behavior == Behavior::Panic,
        },
    )
    .unwrap();
    let recover = || facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &cancellation);
    let outcome = if behavior == Behavior::WrongWorker {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async { recover() })
    } else {
        recover()
    };
    if !matches!(
        behavior,
        Behavior::Panic | Behavior::Stalled | Behavior::CommitUnknown
    ) {
        assert!(
            gate.acquire().is_ok(),
            "closed recovery must leave the gate healthy"
        );
        assert_eq!(
            facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new()),
            Err(Failure::RevisionConflict)
        );
    } else {
        assert!(matches!(gate.acquire(), Err(Failure::StalledWorker)));
    }
    let _ = stop.send(());
    let events = scripted.join().unwrap();
    let rendered = format!("{outcome:?}");
    script::assert_redacted(&rendered);
    assert!(!rendered.contains(WINDOW));
    script::assert_redacted(&test_log_capture::render(&capture.finish()));
    test_log_capture::assert_process_logs_exclude(&[
        script::SECRET,
        script::ATTRIBUTE,
        script::LOCATOR,
        WINDOW,
    ]);
    assert!(!bus.activation_attempted());
    (outcome, events)
}

async fn emit_completion(connection: &Connection, dismissed: bool) {
    emit_completion_at(connection, None, PROMPT_PATH, dismissed).await;
}

async fn emit_completion_at(
    connection: &Connection,
    destination: Option<&str>,
    path: &str,
    dismissed: bool,
) {
    connection
        .emit_signal(
            destination,
            path,
            PROMPT,
            "Completed",
            &(
                dismissed,
                zbus::Variant::from(Vec::<OwnedObjectPath>::new()),
            ),
        )
        .await
        .unwrap();
}

fn interactive_events(events: &[Event]) -> Vec<Event> {
    events
        .iter()
        .copied()
        .filter(|event| !matches!(event, Event::Ordinary(_)))
        .collect()
}

#[test]
fn subscribes_before_prompt_and_retires_before_fresh_verification() {
    for (scope, window) in [
        (RecoveryScope::Item, Some(WINDOW)),
        (RecoveryScope::DefaultCollection, None),
    ] {
        let (outcome, events) = run(Behavior::Accept, scope, window);
        assert_eq!(outcome, Ok(RecoveryOutcome::Ready));
        assert_eq!(
            interactive_events(&events),
            [Event::Unlock, Event::Prompt, Event::Verification]
        );
        if scope == RecoveryScope::Item {
            assert_eq!(events.last(), Some(&Event::Ordinary(script::Call::Close)));
            assert!(!events.contains(&Event::Ordinary(script::Call::Alias)));
        }
    }
}

#[test]
fn dismissal_by_the_user_does_not_verify_or_send_dismiss_again() {
    let (outcome, events) = run(Behavior::UserDismisses, RecoveryScope::Item, None);
    assert_eq!(outcome, Err(Failure::Cancelled));
    assert_eq!(interactive_events(&events), [Event::Unlock, Event::Prompt]);
}

#[test]
fn cancel_and_deadline_dismiss_and_report_unconfirmed_settlement() {
    for (behavior, expected) in [
        (Behavior::CancelOnPrompt, Err(Failure::Cancelled)),
        (Behavior::NeverCompletes, Err(Failure::Unavailable)),
        (Behavior::IgnoresDismiss, Err(Failure::Cancelled)),
        (Behavior::IgnoresDeadline, Err(Failure::Unavailable)),
    ] {
        let (actual, events) = run(behavior, RecoveryScope::Item, None);
        assert_eq!(actual, expected);
        assert_eq!(
            interactive_events(&events),
            [Event::Unlock, Event::Prompt, Event::Dismiss]
        );
    }
}

#[test]
fn immediate_unlock_verifies_and_a_completed_prompt_does_not_prove_unlock() {
    let (outcome, events) = run(Behavior::NoPrompt, RecoveryScope::Item, None);
    assert_eq!(outcome, Ok(RecoveryOutcome::Ready));
    assert_eq!(
        interactive_events(&events),
        [Event::Unlock, Event::Verification]
    );

    for behavior in [Behavior::StillLocked, Behavior::VerificationFails] {
        let (outcome, events) = run(behavior, RecoveryScope::Item, None);
        assert_eq!(outcome, Ok(RecoveryOutcome::RecoveryRequired));
        assert_eq!(
            interactive_events(&events),
            [Event::Unlock, Event::Prompt, Event::Verification]
        );
        if behavior == Behavior::VerificationFails {
            assert_eq!(events.last(), Some(&Event::Ordinary(script::Call::Close)));
        }
    }
    let (outcome, events) = run(Behavior::VerificationDenied, RecoveryScope::Item, None);
    assert_eq!(outcome, Err(Failure::AccessDenied));
    assert_eq!(
        interactive_events(&events),
        [Event::Unlock, Event::Prompt, Event::Verification]
    );
}

#[test]
fn changed_target_never_reaches_interactive_methods() {
    let (outcome, events) = run(Behavior::ChangedTarget, RecoveryScope::Item, None);
    assert_eq!(outcome, Err(Failure::AmbiguousMatch));
    assert!(interactive_events(&events).is_empty());
}

#[test]
fn queued_approval_wins_cancellation_and_is_verified() {
    let (outcome, events) = run(Behavior::QueuedApproval, RecoveryScope::Item, None);
    assert_eq!(outcome, Ok(RecoveryOutcome::Ready));
    assert_eq!(
        interactive_events(&events),
        [Event::Unlock, Event::Prompt, Event::Verification]
    );
}

#[test]
fn queued_completions_win_both_cancellation_and_the_deadline() {
    for behavior in [Behavior::QueuedDismissal, Behavior::DeadlineQueuedDismissal] {
        let (outcome, events) = run(behavior, RecoveryScope::Item, None);
        assert_eq!(outcome, Err(Failure::Cancelled));
        assert_eq!(interactive_events(&events), [Event::Unlock, Event::Prompt]);
    }
    let (outcome, events) = run(Behavior::DeadlineQueuedApproval, RecoveryScope::Item, None);
    assert_eq!(outcome, Ok(RecoveryOutcome::Ready));
    assert_eq!(
        interactive_events(&events),
        [Event::Unlock, Event::Prompt, Event::Verification]
    );
}

#[test]
fn approval_during_dismissal_is_verified_for_cancel_and_deadline() {
    for behavior in [
        Behavior::ApprovesDuringCancel,
        Behavior::ApprovesDuringDeadline,
    ] {
        let (outcome, events) = run(behavior, RecoveryScope::Item, None);
        assert_eq!(outcome, Ok(RecoveryOutcome::Ready));
        assert_eq!(
            interactive_events(&events),
            [
                Event::Unlock,
                Event::Prompt,
                Event::Dismiss,
                Event::Verification
            ]
        );
    }
}

#[test]
fn recovery_refuses_wrong_worker_inside_the_sealed_facade() {
    let (outcome, events) = run(Behavior::WrongWorker, RecoveryScope::Item, None);
    assert_eq!(outcome, Err(Failure::Unsupported));
    assert_eq!(
        events,
        [
            Event::Ordinary(script::Call::Search),
            Event::Ordinary(script::Call::Attributes)
        ]
    );
}

#[test]
fn a_poisoned_boundary_state_fails_closed_without_io() {
    let boundary = Boundary {
        state: Mutex::new(State::Used),
        window: None,
        end: Instant::now(),
        test: TestOptions::default(),
    };
    let _ = std::panic::catch_unwind(|| {
        let _held = boundary.state.lock().unwrap();
        panic!("scripted boundary state poison");
    });
    assert!(matches!(
        boundary.take_pending(),
        Err(RecoveryFailure::Closed(Failure::RevisionConflict))
    ));
}

#[test]
fn forged_completions_from_other_sender_or_path_cannot_approve() {
    for behavior in [Behavior::ForgedSender, Behavior::ForgedPath] {
        let (outcome, events) = run(behavior, RecoveryScope::Item, None);
        assert_eq!(outcome, Err(Failure::Cancelled));
        assert_eq!(interactive_events(&events), [Event::Unlock, Event::Prompt]);
    }
}

#[test]
fn a_prompt_panic_attempts_dismissal_and_reports_possible_open_dialog() {
    let (outcome, events) = run(Behavior::Panic, RecoveryScope::Item, None);
    assert_eq!(outcome, Err(Failure::StalledWorker));
    assert_eq!(
        interactive_events(&events),
        [Event::Unlock, Event::Prompt, Event::Dismiss]
    );
}

#[test]
fn kwallet_and_latched_gates_never_dispatch_recovery() {
    for (behavior, expected) in [
        (Behavior::KWallet, Err(Failure::Unsupported)),
        (Behavior::Stalled, Err(Failure::StalledWorker)),
        (Behavior::CommitUnknown, Err(Failure::StalledWorker)),
    ] {
        let (outcome, events) = run(behavior, RecoveryScope::Item, None);
        assert_eq!(outcome, expected);
        assert!(interactive_events(&events).is_empty());
    }
}
