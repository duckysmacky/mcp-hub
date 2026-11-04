//! Rejects env values that point at internal addresses (SECURITY.md T4).
//!
//! This is validation, not enforcement: the child makes its own connections, so DNS
//! rebinding or redirects can get past it. Errors never include the value.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use url::{Host, Url};

use crate::config::NetworkSettings;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetGuardError {
    #[error("URLs must use https")]
    HttpNotAllowed,
    #[error("the URL has no host")]
    NoHost,
    #[error("the URL's host could not be resolved")]
    Unresolvable,
    #[error("the URL points at an internal address")]
    InternalAddress,
    #[error("this host is only allowed on ports 80 and 443")]
    SelfAddressPort,
}

/// Checks an env value. Anything that isn't an absolute `http(s)://` URL passes.
pub async fn check_env_value(value: &str, rules: &NetworkSettings) -> Result<(), NetGuardError> {
    let Ok(url) = Url::parse(value) else {
        return Ok(());
    };
    match url.scheme() {
        "https" => {}
        "http" if rules.allow_http_urls => {}
        "http" => return Err(NetGuardError::HttpNotAllowed),
        _ => return Ok(()),
    }

    let port = url.port_or_known_default().ok_or(NetGuardError::NoHost)?;
    // `Url` already normalizes IP spellings like `2130706433` and `0x7f.1`.
    let addresses = match url.host().ok_or(NetGuardError::NoHost)? {
        Host::Ipv4(address) => vec![IpAddr::V4(address)],
        Host::Ipv6(address) => vec![IpAddr::V6(address)],
        Host::Domain(domain) => resolve(domain, port).await?,
    };
    check_addresses(&addresses, port, rules)
}

async fn resolve(domain: &str, port: u16) -> Result<Vec<IpAddr>, NetGuardError> {
    let lookup = tokio::net::lookup_host((domain, port));
    let addresses = tokio::time::timeout(RESOLVE_TIMEOUT, lookup)
        .await
        .map_err(|_| NetGuardError::Unresolvable)?
        .map_err(|_| NetGuardError::Unresolvable)?;
    Ok(addresses.map(|address| address.ip()).collect())
}

/// Every address must be acceptable: one bad record is enough to reject.
fn check_addresses(
    addresses: &[IpAddr],
    port: u16,
    rules: &NetworkSettings,
) -> Result<(), NetGuardError> {
    if addresses.is_empty() {
        return Err(NetGuardError::Unresolvable);
    }
    for address in addresses {
        let address = address.to_canonical();
        if rules.self_addresses.contains(&address) {
            if port != 443 && port != 80 {
                return Err(NetGuardError::SelfAddressPort);
            }
        } else if is_internal(address) {
            return Err(NetGuardError::InternalAddress);
        }
    }
    Ok(())
}

fn is_internal(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_internal_v4(address),
        IpAddr::V6(address) => is_internal_v6(address),
    }
}

fn is_internal_v4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_broadcast()
        || address.is_documentation()
        || first == 0
        || first >= 240
        || (first == 100 && (64..128).contains(&second)) // CGNAT
        || (first == 192 && second == 0 && third == 0) // IETF protocol assignments
        || (first == 198 && (18..20).contains(&second)) // benchmarking
}

fn is_internal_v6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    let first = segments[0];
    address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local
        || (first & 0xffc0) == 0xfe80 // link local
        || (first & 0xffc0) == 0xfec0 // deprecated site local
        || (first == 0x2001 && segments[1] == 0x0db8) // documentation
        // These embed an IPv4 address that could be internal.
        || (first == 0x2001 && segments[1] == 0) // Teredo
        || first == 0x2002 // 6to4
        || (first == 0x0064 && segments[1] == 0xff9b) // NAT64
        || segments[..6].iter().all(|&segment| segment == 0) // IPv4-compatible
        || (first == 0x0100 && segments[1..4].iter().all(|&segment| segment == 0)) // discard
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> NetworkSettings {
        NetworkSettings {
            self_addresses: vec!["203.0.113.10".parse().unwrap()],
            allow_http_urls: false,
        }
    }

    #[tokio::test]
    async fn internal_addresses_are_rejected_however_they_are_written() {
        for value in [
            "https://127.0.0.1",
            "https://127.0.0.2",
            "https://127.1",
            "https://2130706433",
            "https://0x7f.1",
            "https://0177.0.0.1",
            "https://0.0.0.0",
            "https://[::1]",
            "https://[::]",
            "https://[::ffff:127.0.0.1]",
            "https://[::ffff:7f00:1]",
            "https://[::127.0.0.1]",
            "https://[64:ff9b::7f00:1]",
            "https://[2002:7f00:1::]",
            "https://[2001:0:7f00:1::]",
            "https://10.0.0.1",
            "https://172.16.0.1",
            "https://172.31.255.255",
            "https://192.168.1.1",
            "https://100.64.0.1",
            "https://100.127.255.255",
            "https://169.254.169.254",
            "https://[fe80::1]",
            "https://[fc00::1]",
            "https://[fd12:3456::1]",
            "https://[ff02::1]",
            "https://224.0.0.1",
            "https://255.255.255.255",
            "https://240.0.0.1",
            "https://192.0.2.1",
            "https://198.51.100.1",
            "https://[2001:db8::1]",
            "https://198.18.0.1",
            "https://user:pass@127.0.0.1/path",
            "https://LOCALHOST",
            "https://localhost:8080/x",
        ] {
            assert_eq!(
                check_env_value(value, &rules()).await,
                Err(NetGuardError::InternalAddress),
                "{value}"
            );
        }
    }

    #[tokio::test]
    async fn public_addresses_pass() {
        for value in [
            "https://1.1.1.1",
            "https://8.8.8.8:8443/path",
            "https://[2606:4700:4700::1111]",
            "https://100.128.0.1",
            "https://172.32.0.1",
        ] {
            assert_eq!(check_env_value(value, &rules()).await, Ok(()), "{value}");
        }
    }

    #[tokio::test]
    async fn plain_http_needs_the_setting() {
        assert_eq!(
            check_env_value("http://1.1.1.1", &rules()).await,
            Err(NetGuardError::HttpNotAllowed)
        );
        let allowed = NetworkSettings {
            allow_http_urls: true,
            ..rules()
        };
        assert_eq!(check_env_value("http://1.1.1.1", &allowed).await, Ok(()));
        assert_eq!(
            check_env_value("http://127.0.0.1", &allowed).await,
            Err(NetGuardError::InternalAddress)
        );
    }

    #[tokio::test]
    async fn non_url_values_pass() {
        for value in [
            "",
            "secret-token",
            "true",
            "default",
            "/a/path",
            "ftp://127.0.0.1",
            "127.0.0.1",
        ] {
            assert_eq!(check_env_value(value, &rules()).await, Ok(()), "{value}");
        }
    }

    #[tokio::test]
    async fn own_public_address_is_only_allowed_on_web_ports() {
        for value in ["https://203.0.113.10", "https://203.0.113.10:80/"] {
            assert_eq!(check_env_value(value, &rules()).await, Ok(()), "{value}");
        }
        assert_eq!(
            check_env_value("https://203.0.113.10:5432", &rules()).await,
            Err(NetGuardError::SelfAddressPort)
        );
    }

    #[tokio::test]
    async fn unresolvable_host_is_rejected() {
        assert_eq!(
            check_env_value("https://does-not-exist.invalid", &rules()).await,
            Err(NetGuardError::Unresolvable)
        );
    }

    #[test]
    fn one_internal_record_among_public_ones_is_enough() {
        let addresses = ["1.1.1.1".parse().unwrap(), "10.0.0.1".parse().unwrap()];
        assert_eq!(
            check_addresses(&addresses, 443, &rules()),
            Err(NetGuardError::InternalAddress)
        );
        assert_eq!(
            check_addresses(&[], 443, &rules()),
            Err(NetGuardError::Unresolvable)
        );
    }

    #[test]
    fn mapped_self_address_is_recognized() {
        let mapped = ["::ffff:203.0.113.10".parse().unwrap()];
        assert_eq!(check_addresses(&mapped, 443, &rules()), Ok(()));
    }

    #[tokio::test]
    async fn errors_never_contain_the_value() {
        let error = check_env_value("https://hunter2:pw@127.0.0.1", &rules())
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("hunter2"));
    }
}
