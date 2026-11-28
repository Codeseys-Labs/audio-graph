use super::*;
use crate::credentials::adapters::linux_secret_service_wire::{
    RecoveryScope, locked_target_at, scripted_peer as script,
};
use crate::test_log_capture;
use std::sync::mpsc;
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Ordinary(script::Call),
    Unlock,
    Prompt,
    Dismiss,
    Verification,
}

fn run(behavior: Behavior, scope: RecoveryScope, window: Option<&str>) -> (Outcome, Vec<Event>) {
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
            let connection = Builder::address(address).build().await.unwrap();
            let mut messages = MessageStream::from(&connection);
            connection.request_name(SERVICE_NAME).await.unwrap();
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
                    (Some(SERVICE), Some("Unlock")) => {
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
                    (Some(PROMPT), Some("Prompt")) => {
                        assert_eq!(path, PROMPT_PATH);
                        assert_eq!(interactive.as_deref(), Some(sender));
                        assert!(!shown);
                        let window: String = message.body().deserialize().unwrap();
                        assert!(window == expected_window);
                        shown = true;
                        if matches!(
                            behavior,
                            Behavior::Accept
                                | Behavior::UserDismisses
                                | Behavior::StillLocked
                                | Behavior::VerificationFails
                                | Behavior::VerificationDenied
                        ) {
                            unlocked = matches!(
                                behavior,
                                Behavior::Accept
                                    | Behavior::VerificationFails
                                    | Behavior::VerificationDenied
                            );
                            // Broadcast and emit before replying. The private bus
                            // only routes this signal if AddMatch preceded Prompt.
                            emit_completion(&connection, behavior == Behavior::UserDismisses).await;
                            shown = false;
                        }
                        connection.reply(&header, &()).await.unwrap();
                        if matches!(
                            behavior,
                            Behavior::CancelOnPrompt | Behavior::IgnoresDismiss
                        ) {
                            cancellation_for_peer.cancel();
                        }
                        Event::Prompt
                    }
                    (Some(PROMPT), Some("Dismiss")) => {
                        assert_eq!(path, PROMPT_PATH);
                        assert_eq!(interactive.as_deref(), Some(sender));
                        assert!(shown);
                        if behavior != Behavior::IgnoresDismiss {
                            emit_completion(&connection, true).await;
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
            assert_eq!(shown, behavior == Behavior::IgnoresDismiss);
            events
        })
    });
    response.recv_timeout(script::GUARD).unwrap();
    let gate = NativeInteractionGate::isolated_for_test();
    let target = {
        let lease = gate.acquire().unwrap();
        locked_target_at(
            bus.address(),
            lease.forbid_prompt(),
            script::wanted(),
            scope,
        )
        .unwrap()
        .expect("an exact locked target")
    };
    let budget = if behavior == Behavior::NeverCompletes {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(4)
    };
    let outcome = recover_exact(
        gate,
        target,
        window.map(str::to_owned),
        budget,
        &cancellation,
    );
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
    connection
        .emit_signal(
            None::<&str>,
            PROMPT_PATH,
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
fn subscribes_before_prompt_and_verifies_encrypted_read_on_a_retired_connection() {
    for (scope, window) in [
        (RecoveryScope::Item, Some(WINDOW)),
        (RecoveryScope::DefaultCollection, None),
    ] {
        let (outcome, events) = run(Behavior::Accept, scope, window);
        assert_eq!(outcome, Outcome::Verified);
        assert_eq!(
            interactive_events(&events),
            [Event::Unlock, Event::Prompt, Event::Verification]
        );
        assert_eq!(events.last(), Some(&Event::Ordinary(script::Call::Close)));
    }
}

#[test]
fn dismissal_by_the_user_does_not_verify_or_send_dismiss_again() {
    let (outcome, events) = run(Behavior::UserDismisses, RecoveryScope::Item, None);
    assert_eq!(outcome, Outcome::DismissedByUser);
    assert_eq!(interactive_events(&events), [Event::Unlock, Event::Prompt]);
}

#[test]
fn cancel_and_deadline_dismiss_and_report_unconfirmed_settlement() {
    for (behavior, expected) in [
        (
            Behavior::CancelOnPrompt,
            Outcome::Cancelled(Dismissal::Completed),
        ),
        (
            Behavior::NeverCompletes,
            Outcome::TimedOut(Dismissal::Completed),
        ),
        (
            Behavior::IgnoresDismiss,
            Outcome::Cancelled(Dismissal::Unconfirmed),
        ),
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
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(
        interactive_events(&events),
        [Event::Unlock, Event::Verification]
    );

    for behavior in [Behavior::StillLocked, Behavior::VerificationFails] {
        let (outcome, events) = run(behavior, RecoveryScope::Item, None);
        assert_eq!(outcome, Outcome::StillLocked);
        assert_eq!(
            interactive_events(&events),
            [Event::Unlock, Event::Prompt, Event::Verification]
        );
        if behavior == Behavior::VerificationFails {
            assert_eq!(events.last(), Some(&Event::Ordinary(script::Call::Close)));
        }
    }
    let (outcome, events) = run(Behavior::VerificationDenied, RecoveryScope::Item, None);
    assert_eq!(outcome, Outcome::Failed(Failure::AccessDenied));
    assert_eq!(
        interactive_events(&events),
        [Event::Unlock, Event::Prompt, Event::Verification]
    );
}

#[test]
fn changed_target_never_reaches_interactive_methods() {
    let (outcome, events) = run(Behavior::ChangedTarget, RecoveryScope::Item, None);
    assert_eq!(outcome, Outcome::Failed(Failure::AmbiguousMatch));
    assert!(interactive_events(&events).is_empty());
}
