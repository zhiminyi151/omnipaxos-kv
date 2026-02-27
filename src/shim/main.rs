use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    str::FromStr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use log::{error, info, warn};
use omnipaxos_kv::common::{
    kv::{CommandId, KVCommand, NodeId},
    messages::{ClientMessage, RegistrationMessage, ServerMessage},
    utils::{
        frame_clients_connection, frame_registration_connection, FromServerConnection,
        ToServerConnection,
    },
};
use serde::{Deserialize, Serialize};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot, Mutex},
    time::timeout,
};
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct AppState {
    client: Arc<ShimClient>,
}

struct ShimClient {
    next_command_id: AtomicUsize,
    request_timeout: Duration,
    to_server: mpsc::Sender<ClientMessage>,
    pending: Arc<Mutex<HashMap<CommandId, oneshot::Sender<ServerMessage>>>>,
}

impl ShimClient {
    async fn connect(server_id: NodeId, server_address: String, request_timeout: Duration) -> Self {
        let stream = loop {
            match TcpStream::connect(&server_address).await {
                Ok(s) => break s,
                Err(e) => {
                    warn!("Failed connecting to server {} at {}: {}", server_id, server_address, e);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        };
        stream.set_nodelay(true).expect("set_nodelay failed");
        let mut registration_connection = frame_registration_connection(stream);
        registration_connection
            .send(RegistrationMessage::ClientRegister)
            .await
            .expect("failed to send client registration");
        let underlying_stream = registration_connection.into_inner().into_inner();
        let (from_server, to_server) = frame_clients_connection(underlying_stream);

        let pending: Arc<Mutex<HashMap<CommandId, oneshot::Sender<ServerMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (outgoing_tx, outgoing_rx) = mpsc::channel(128);

        Self::spawn_writer_task(outgoing_rx, to_server);
        Self::spawn_reader_task(from_server, pending.clone());

        Self {
            next_command_id: AtomicUsize::new(1),
            request_timeout,
            to_server: outgoing_tx,
            pending,
        }
    }

    fn spawn_writer_task(
        mut outgoing_rx: mpsc::Receiver<ClientMessage>,
        mut to_server: ToServerConnection,
    ) {
        tokio::spawn(async move {
            while let Some(msg) = outgoing_rx.recv().await {
                if let Err(e) = to_server.feed(msg).await {
                    error!("shim writer failed to feed message: {}", e);
                    break;
                }
                if let Err(e) = to_server.flush().await {
                    error!("shim writer failed to flush message: {}", e);
                    break;
                }
            }
        });
    }

    fn spawn_reader_task(
        mut from_server: FromServerConnection,
        pending: Arc<Mutex<HashMap<CommandId, oneshot::Sender<ServerMessage>>>>,
    ) {
        tokio::spawn(async move {
            while let Some(msg) = from_server.next().await {
                match msg {
                    Ok(ServerMessage::StartSignal(_)) => {
                        // The benchmark client waits for this signal, but the shim can ignore it.
                    }
                    Ok(server_msg) => {
                        let cmd_id = server_msg.command_id();
                        let maybe_sender = {
                            let mut pending_locked = pending.lock().await;
                            pending_locked.remove(&cmd_id)
                        };
                        if let Some(sender) = maybe_sender {
                            let _ = sender.send(server_msg);
                        } else {
                            warn!("No pending request for command id {}", cmd_id);
                        }
                    }
                    Err(e) => {
                        error!("shim reader failed to deserialize server message: {}", e);
                    }
                }
            }
        });
    }

    async fn send_command(&self, command: KVCommand) -> Result<ServerMessage, ShimError> {
        let command_id = self.next_command_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending_locked = self.pending.lock().await;
            pending_locked.insert(command_id, tx);
        }

        if self
            .to_server
            .send(ClientMessage::Append(command_id, command))
            .await
            .is_err()
        {
            let mut pending_locked = self.pending.lock().await;
            pending_locked.remove(&command_id);
            return Err(ShimError::Disconnected);
        }

        match timeout(self.request_timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(ShimError::Disconnected),
            Err(_) => {
                let mut pending_locked = self.pending.lock().await;
                pending_locked.remove(&command_id);
                Err(ShimError::Timeout)
            }
        }
    }
}

#[derive(Debug)]
enum ShimError {
    Timeout,
    Disconnected,
}

#[derive(Serialize)]
struct PutResponse {
    ok: bool,
}

#[derive(Serialize)]
struct GetResponse {
    ok: bool,
    value: Option<String>,
}

#[derive(Serialize)]
struct CasResponse {
    ok: bool,
    applied: bool,
}

#[derive(Serialize)]
struct ErrorResponse {
    ok: bool,
    indeterminate: bool,
    error: String,
}

#[derive(Deserialize)]
struct PutBody {
    value: String,
}

#[derive(Deserialize)]
struct CasBody {
    key: String,
    from: String,
    to: String,
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let server_id: NodeId = env::var("SHIM_SERVER_ID")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let server_address =
        env::var("SHIM_SERVER_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8001".to_string());
    let bind_address = env::var("SHIM_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let timeout_ms: u64 = env::var("SHIM_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);

    let shim_client =
        ShimClient::connect(server_id, server_address.clone(), Duration::from_millis(timeout_ms))
            .await;
    let app_state = AppState {
        client: Arc::new(shim_client),
    };

    let app = Router::new()
        .route("/kv/:key", put(put_value))
        .route("/kv/:key", get(get_value))
        .route("/kv/cas", post(cas_value))
        .with_state(app_state)
        .layer(CorsLayer::permissive());

    let socket_addr = SocketAddr::from_str(&bind_address).expect("invalid SHIM_BIND_ADDRESS");
    info!(
        "shim listening on {} and forwarding to server {} ({})",
        socket_addr, server_id, server_address
    );
    let listener = tokio::net::TcpListener::bind(socket_addr)
        .await
        .expect("failed to bind shim HTTP listener");
    axum::serve(listener, app).await.expect("shim server error");
}

async fn put_value(
    Path(key): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<PutBody>,
) -> impl IntoResponse {
    match state
        .client
        .send_command(KVCommand::Put(key, body.value))
        .await
    {
        Ok(ServerMessage::Write(_)) => (StatusCode::OK, Json(PutResponse { ok: true })).into_response(),
        Ok(other) => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                ok: false,
                indeterminate: false,
                error: format!("unexpected response for put: {:?}", other),
            }),
        )
            .into_response(),
        Err(err) => shim_error_response(err),
    }
}

async fn get_value(Path(key): Path<String>, State(state): State<AppState>) -> impl IntoResponse {
    match state.client.send_command(KVCommand::Get(key)).await {
        Ok(ServerMessage::Read(_, value)) => {
            (StatusCode::OK, Json(GetResponse { ok: true, value })).into_response()
        }
        Ok(other) => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                ok: false,
                indeterminate: false,
                error: format!("unexpected response for get: {:?}", other),
            }),
        )
            .into_response(),
        Err(err) => shim_error_response(err),
    }
}

async fn cas_value(
    State(state): State<AppState>,
    Json(body): Json<CasBody>,
) -> impl IntoResponse {
    match state
        .client
        .send_command(KVCommand::Cas(body.key, body.from, body.to))
        .await
    {
        Ok(ServerMessage::CasOk(_)) => (
            StatusCode::OK,
            Json(CasResponse {
                ok: true,
                applied: true,
            }),
        )
            .into_response(),
        Ok(ServerMessage::CasFail(_)) => (
            StatusCode::OK,
            Json(CasResponse {
                ok: true,
                applied: false,
            }),
        )
            .into_response(),
        Ok(other) => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                ok: false,
                indeterminate: false,
                error: format!("unexpected response for cas: {:?}", other),
            }),
        )
            .into_response(),
        Err(err) => shim_error_response(err),
    }
}

fn shim_error_response(err: ShimError) -> axum::response::Response {
    match err {
        ShimError::Timeout => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(ErrorResponse {
                ok: false,
                indeterminate: true,
                error: "request timed out; operation may or may not have succeeded".to_string(),
            }),
        )
            .into_response(),
        ShimError::Disconnected => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                ok: false,
                indeterminate: true,
                error: "connection to server lost".to_string(),
            }),
        )
            .into_response(),
    }
}
