//! Request limits for the endpoints that can be brute-forced or used to fill the database,
//! and working out which address a request really came from.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderName},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{app_state::AppState, config::Cidr, error::ApiError};

const WINDOW: Duration = Duration::from_secs(60);
const MAX_TRACKED_KEYS: usize = 100_000;
const FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");

/// When each bucket and key's current window started, and the requests counted in it.
type Windows = HashMap<(&'static str, String), (Instant, u32)>;

/// Fixed-window counters.
// ponytail: per process and reset by a restart, and a burst can straddle two windows. That's
// fine for one hub behind one proxy; move this to Postgres or Redis if the hub ever scales out.
#[derive(Default)]
pub struct RateLimiter {
    windows: Mutex<Windows>,
    warned_about_proxy: AtomicBool,
}

impl RateLimiter {
    /// Counts one request. Over the limit, returns the seconds until the window ends.
    pub fn check(&self, bucket: &'static str, key: &str, limit: u32) -> Result<(), ApiError> {
        let now = Instant::now();
        let mut windows = self
            .windows
            .lock()
            .expect("no code panics holding this lock");
        if windows.len() >= MAX_TRACKED_KEYS {
            windows.retain(|_, (started, _)| now.duration_since(*started) < WINDOW);
        }
        // Still full means a flood of distinct keys: start over instead of growing forever.
        if windows.len() >= MAX_TRACKED_KEYS {
            windows.clear();
        }
        let (started, count) = windows.entry((bucket, key.to_owned())).or_insert((now, 0));
        if now.duration_since(*started) >= WINDOW {
            *started = now;
            *count = 0;
        }
        if *count >= limit {
            let remaining = WINDOW.saturating_sub(now.duration_since(*started));
            return Err(ApiError::RateLimited {
                retry_after_secs: remaining.as_secs() + 1,
            });
        }
        *count += 1;
        Ok(())
    }
}

/// Who sent the request: the peer, or, when the peer is a trusted proxy, the nearest address
/// it reports that isn't another trusted proxy. Entries further left can be forged by the
/// client, so the walk starts from the right.
pub fn resolve_client_ip(peer: IpAddr, headers: &HeaderMap, trusted: &[Cidr]) -> IpAddr {
    let is_trusted = |address: IpAddr| trusted.iter().any(|range| range.contains(address));
    let peer = peer.to_canonical();
    if !is_trusted(peer) {
        return peer;
    }
    let mut client = peer;
    let forwarded = headers
        .get_all(FORWARDED_FOR)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .collect::<Vec<_>>();
    for entry in forwarded.into_iter().rev() {
        let Ok(address) = entry.trim().parse::<IpAddr>() else {
            break;
        };
        client = address.to_canonical();
        if !is_trusted(client) {
            break;
        }
    }
    client
}

/// A rate limit key for an address. One IPv6 customer usually holds a whole /64.
pub fn ip_key(address: IpAddr) -> String {
    match address {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => {
            let segments = address.segments();
            format!(
                "{:x}:{:x}:{:x}:{:x}::/64",
                segments[0], segments[1], segments[2], segments[3]
            )
        }
    }
}

/// The key for this request's client. Without connection info, as in router-only tests,
/// every request shares one key.
fn request_key(state: &AppState, request: &Request) -> String {
    let Some(ConnectInfo(peer)) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return "unknown".to_owned();
    };
    let trusted = &state.config.settings.network.trusted_proxies;
    if request.headers().contains_key(FORWARDED_FOR)
        && !trusted.iter().any(|range| range.contains(peer.ip()))
        && !state
            .rate_limiter
            .warned_about_proxy
            .swap(true, Ordering::Relaxed)
    {
        tracing::warn!(
            peer = %peer.ip(),
            "X-Forwarded-For from an untrusted peer; behind a proxy, set network.trusted_proxies \
             or every client shares one rate limit"
        );
    }
    ip_key(resolve_client_ip(peer.ip(), request.headers(), trusted))
}

/// Middleware state: the app, then the bucket's name and its limit per address per minute.
pub type LimitState = (AppState, &'static str, u32);

pub async fn limit_by_ip(
    State((state, bucket, limit)): State<LimitState>,
    request: Request,
    next: Next,
) -> Response {
    let key = request_key(&state, &request);
    match state.rate_limiter.check(bucket, &key, limit) {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn address(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn forwarded(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(FORWARDED_FOR, HeaderValue::from_str(value).unwrap());
        headers
    }

    fn trusted() -> Vec<Cidr> {
        vec![Cidr::try_from("172.16.0.0/12".to_owned()).unwrap()]
    }

    #[test]
    fn an_untrusted_peer_cannot_claim_an_address() {
        assert_eq!(
            resolve_client_ip(address("203.0.113.9"), &forwarded("1.1.1.1"), &trusted()),
            address("203.0.113.9")
        );
    }

    #[test]
    fn a_trusted_proxy_reports_the_client() {
        assert_eq!(
            resolve_client_ip(
                address("172.17.0.1"),
                &forwarded("198.51.100.7"),
                &trusted()
            ),
            address("198.51.100.7")
        );
        // What the client sent sits on the left; the proxy appended the real one.
        assert_eq!(
            resolve_client_ip(
                address("172.17.0.1"),
                &forwarded("1.1.1.1, 198.51.100.7"),
                &trusted()
            ),
            address("198.51.100.7")
        );
        // A chain of proxies is skipped.
        assert_eq!(
            resolve_client_ip(
                address("172.17.0.1"),
                &forwarded("198.51.100.7, 172.18.0.2"),
                &trusted()
            ),
            address("198.51.100.7")
        );
    }

    #[test]
    fn garbage_or_no_header_falls_back_safely() {
        let peer = address("172.17.0.1");
        assert_eq!(resolve_client_ip(peer, &HeaderMap::new(), &trusted()), peer);
        assert_eq!(
            resolve_client_ip(peer, &forwarded("not-an-ip"), &trusted()),
            peer
        );
        assert_eq!(
            resolve_client_ip(peer, &forwarded("1.1.1.1, not-an-ip"), &trusted()),
            peer
        );
    }

    #[test]
    fn mapped_addresses_count_as_their_ipv4_address() {
        assert_eq!(
            resolve_client_ip(
                address("::ffff:172.17.0.1"),
                &forwarded("198.51.100.7"),
                &trusted()
            ),
            address("198.51.100.7")
        );
    }

    #[test]
    fn ipv6_clients_are_keyed_by_their_network() {
        assert_eq!(
            ip_key(address("2001:db8:1:2:aaaa::1")),
            ip_key(address("2001:db8:1:2:bbbb::2"))
        );
        assert_ne!(
            ip_key(address("2001:db8:1:2::1")),
            ip_key(address("2001:db8:1:3::1"))
        );
        assert_eq!(ip_key(address("192.0.2.1")), "192.0.2.1");
    }

    #[test]
    fn limits_are_per_bucket_and_key() {
        let limiter = RateLimiter::default();
        assert!(limiter.check("a", "x", 2).is_ok());
        assert!(limiter.check("a", "x", 2).is_ok());
        assert!(matches!(
            limiter.check("a", "x", 2),
            Err(ApiError::RateLimited { retry_after_secs }) if retry_after_secs <= 61
        ));
        assert!(limiter.check("a", "y", 2).is_ok());
        assert!(limiter.check("b", "x", 2).is_ok());
    }

    #[test]
    fn the_map_cannot_grow_past_its_bound() {
        let limiter = RateLimiter::default();
        for index in 0..=MAX_TRACKED_KEYS {
            limiter.check("a", &index.to_string(), 1).unwrap();
        }
        assert!(limiter.windows.lock().unwrap().len() <= MAX_TRACKED_KEYS);
    }
}
