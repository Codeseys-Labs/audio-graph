//! Test-only WebSocket fixture helpers for streaming ASR providers.

use std::future::Future;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, accept_async, connect_async};

pub(super) type ServerSocket = WebSocketStream<TcpStream>;
pub(super) type ClientSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const EXPECT_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClientFrame {
    Text(String),
    Binary(Vec<u8>),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ServerStep {
    SendText(String),
    SendBinary(Vec<u8>),
    SendClose,
    ExpectText(String),
    ExpectBinary(Vec<u8>),
    ExpectAnyText,
    ExpectAnyBinary,
    ExpectClose,
}

impl ServerStep {
    pub(super) fn send_text(text: impl Into<String>) -> Self {
        Self::SendText(text.into())
    }

    pub(super) fn send_binary(bytes: impl Into<Vec<u8>>) -> Self {
        Self::SendBinary(bytes.into())
    }

    pub(super) const fn send_close() -> Self {
        Self::SendClose
    }

    pub(super) fn expect_text(text: impl Into<String>) -> Self {
        Self::ExpectText(text.into())
    }

    pub(super) fn expect_binary(bytes: impl Into<Vec<u8>>) -> Self {
        Self::ExpectBinary(bytes.into())
    }

    pub(super) const fn expect_any_text() -> Self {
        Self::ExpectAnyText
    }

    pub(super) const fn expect_any_binary() -> Self {
        Self::ExpectAnyBinary
    }

    pub(super) const fn expect_close() -> Self {
        Self::ExpectClose
    }
}

pub(super) async fn spawn_server<F, Fut, T>(handler: F) -> (String, JoinHandle<T>)
where
    F: FnOnce(ServerSocket) -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local websocket server");
    let addr = listener.local_addr().expect("local websocket addr");

    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket");
        let websocket = accept_async(stream)
            .await
            .expect("server websocket handshake");
        handler(websocket).await
    });

    (format!("ws://{addr}"), handle)
}

pub(super) async fn spawn_scripted_server(
    steps: Vec<ServerStep>,
) -> (String, JoinHandle<Vec<ClientFrame>>) {
    spawn_server(|websocket| run_scripted_server(websocket, steps)).await
}

pub(super) async fn connect_client(url: &str) -> ClientSocket {
    let (socket, _) = connect_async(url).await.expect("client websocket connect");
    socket
}

async fn run_scripted_server(
    mut websocket: ServerSocket,
    steps: Vec<ServerStep>,
) -> Vec<ClientFrame> {
    let mut received = Vec::new();

    for step in steps {
        match step {
            ServerStep::SendText(text) => websocket
                .send(Message::Text(text.into()))
                .await
                .expect("scripted server sends text"),
            ServerStep::SendBinary(bytes) => websocket
                .send(Message::Binary(bytes.into()))
                .await
                .expect("scripted server sends binary"),
            ServerStep::SendClose => websocket
                .close(None)
                .await
                .expect("scripted server sends close"),
            ServerStep::ExpectText(expected) => {
                let actual = recv_client_frame(&mut websocket).await;
                assert_eq!(
                    actual,
                    ClientFrame::Text(expected),
                    "scripted server received unexpected text frame"
                );
                received.push(actual);
            }
            ServerStep::ExpectBinary(expected) => {
                let actual = recv_client_frame(&mut websocket).await;
                assert_eq!(
                    actual,
                    ClientFrame::Binary(expected),
                    "scripted server received unexpected binary frame"
                );
                received.push(actual);
            }
            ServerStep::ExpectAnyText => {
                let actual = recv_client_frame(&mut websocket).await;
                assert!(
                    matches!(actual, ClientFrame::Text(_)),
                    "scripted server expected any text frame, got {actual:?}"
                );
                received.push(actual);
            }
            ServerStep::ExpectAnyBinary => {
                let actual = recv_client_frame(&mut websocket).await;
                assert!(
                    matches!(actual, ClientFrame::Binary(_)),
                    "scripted server expected any binary frame, got {actual:?}"
                );
                received.push(actual);
            }
            ServerStep::ExpectClose => {
                let actual = recv_client_frame(&mut websocket).await;
                assert_eq!(
                    actual,
                    ClientFrame::Close,
                    "scripted server expected close frame"
                );
                received.push(actual);
            }
        }
    }

    received
}

async fn recv_client_frame(websocket: &mut ServerSocket) -> ClientFrame {
    let frame = tokio::time::timeout(EXPECT_FRAME_TIMEOUT, websocket.next())
        .await
        .expect("timed out waiting for scripted client frame")
        .expect("client closed before scripted server received expected frame")
        .expect("scripted server received client frame");

    match frame {
        Message::Text(text) => ClientFrame::Text(text.to_string()),
        Message::Binary(bytes) => ClientFrame::Binary(bytes.to_vec()),
        Message::Close(_) => ClientFrame::Close,
        other => panic!("scripted server expected text/binary/close frame, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fake_websocket_server_round_trips_text_and_binary() {
    let (url, server) = spawn_server(|mut websocket| async move {
        websocket
            .send(Message::Text("ready".into()))
            .await
            .expect("server sends ready");
        match websocket.next().await.expect("server receives frame") {
            Ok(Message::Binary(bytes)) => bytes.to_vec(),
            other => panic!("expected binary frame, got {other:?}"),
        }
    })
    .await;

    let mut client = connect_client(&url).await;
    match client.next().await.expect("client receives ready") {
        Ok(Message::Text(text)) => assert_eq!(text, "ready"),
        other => panic!("expected ready text frame, got {other:?}"),
    }
    client
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .expect("client sends binary");

    let bytes = tokio::time::timeout(std::time::Duration::from_secs(1), server)
        .await
        .expect("server task finishes")
        .expect("server task panicked");
    assert_eq!(bytes, vec![1, 2, 3]);
}

#[tokio::test(flavor = "current_thread")]
async fn scripted_fake_server_sends_frames_and_captures_client_frames() {
    let (url, server) = spawn_scripted_server(vec![
        ServerStep::send_text("ready"),
        ServerStep::expect_any_binary(),
        ServerStep::expect_any_text(),
        ServerStep::send_binary(vec![4, 5]),
    ])
    .await;

    let mut client = connect_client(&url).await;
    match client.next().await.expect("client receives ready") {
        Ok(Message::Text(text)) => assert_eq!(text, "ready"),
        other => panic!("expected ready text frame, got {other:?}"),
    }
    client
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .expect("client sends binary");
    client
        .send(Message::Text(r#"{"type":"Terminate"}"#.into()))
        .await
        .expect("client sends text");
    match client.next().await.expect("client receives binary") {
        Ok(Message::Binary(bytes)) => assert_eq!(bytes.as_ref(), &[4, 5]),
        other => panic!("expected binary frame, got {other:?}"),
    }

    let received = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("server task finishes")
        .expect("server task panicked");
    assert_eq!(
        received,
        vec![
            ClientFrame::Binary(vec![1, 2, 3]),
            ClientFrame::Text(r#"{"type":"Terminate"}"#.into()),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn scripted_fake_server_can_assert_close_frames() {
    let (url, server) = spawn_scripted_server(vec![ServerStep::expect_close()]).await;

    let mut client = connect_client(&url).await;
    client
        .send(Message::Close(None))
        .await
        .expect("client sends close frame");

    let received = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("server task finishes")
        .expect("server task panicked");
    assert_eq!(received, vec![ClientFrame::Close]);
}

#[tokio::test(flavor = "current_thread")]
async fn scripted_fake_server_can_assert_exact_client_frames() {
    let (url, server) = spawn_scripted_server(vec![
        ServerStep::expect_binary(vec![9, 8]),
        ServerStep::expect_text("exact terminal"),
    ])
    .await;

    let mut client = connect_client(&url).await;
    client
        .send(Message::Binary(vec![9, 8].into()))
        .await
        .expect("client sends binary");
    client
        .send(Message::Text("exact terminal".into()))
        .await
        .expect("client sends text");

    let received = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("server task finishes")
        .expect("server task panicked");
    assert_eq!(
        received,
        vec![
            ClientFrame::Binary(vec![9, 8]),
            ClientFrame::Text("exact terminal".into()),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn scripted_fake_server_can_send_close_frames() {
    let (url, server) = spawn_scripted_server(vec![ServerStep::send_close()]).await;

    let mut client = connect_client(&url).await;
    match client.next().await.expect("client receives close") {
        Ok(Message::Close(_)) => {}
        other => panic!("expected close frame, got {other:?}"),
    }

    let received = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("server task finishes")
        .expect("server task panicked");
    assert!(received.is_empty());
}
