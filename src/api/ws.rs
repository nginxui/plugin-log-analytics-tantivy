//! WebSocket upgrade for the plugin streams. The host proxies the upgrade, the
//! plugin answers the handshake and then speaks frames on the upgraded
//! connection.

use std::future::Future;

use hyper_util::rt::TokioIo;
use nginxui_plugin_sdk::http::{full_body, header, hyper, HeaderValue, Incoming, Request, Response, StatusCode};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::WebSocketStream;

use super::respond::Resp;

pub type Socket = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;

fn plain(status: StatusCode, text: &'static str) -> Resp {
    let mut response = Response::new(full_body(text));
    *response.status_mut() = status;
    response
}

fn has_token(req: &Request<Incoming>, name: header::HeaderName, token: &str) -> bool {
    req.headers()
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
}

/// Answers the handshake and runs `handler` on the socket once the connection
/// is upgraded. A request that is not a WebSocket upgrade gets a 400.
///
/// The origin is not checked: the host authenticated the request and checked
/// its origin before it proxied it, and on the private socket the origin no
/// longer matches the host name.
pub fn upgrade<F, Fut>(mut req: Request<Incoming>, handler: F) -> Resp
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if !has_token(&req, header::CONNECTION, "upgrade") || !has_token(&req, header::UPGRADE, "websocket") {
        return plain(StatusCode::BAD_REQUEST, "websocket upgrade expected");
    }
    if req.headers().get(header::SEC_WEBSOCKET_VERSION).and_then(|v| v.to_str().ok()) != Some("13") {
        let mut response = plain(StatusCode::BAD_REQUEST, "unsupported websocket version");
        response.headers_mut().insert(header::SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
        return response;
    }
    let Some(key) = req.headers().get(header::SEC_WEBSOCKET_KEY).map(|k| k.as_bytes().to_vec()) else {
        return plain(StatusCode::BAD_REQUEST, "missing websocket key");
    };
    let accept = derive_accept_key(&key);
    let on_upgrade = hyper::upgrade::on(&mut req);

    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let socket = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
                handler(socket).await;
            }
            Err(e) => nginxui_plugin_sdk::debug!("websocket upgrade failed: {e}"),
        }
    });

    let mut response = Response::new(full_body(Vec::new()));
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    if let Ok(v) = HeaderValue::from_str(&accept) {
        headers.insert(header::SEC_WEBSOCKET_ACCEPT, v);
    }
    response
}
