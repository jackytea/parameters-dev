use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::{Attempt, Policy};
use reqwest::{Client, Url};

const MAX_REDIRECTS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sandbox {
    pub privileged_nodes: bool,
    pub private_network: bool,
}

impl Sandbox {
    pub const OPEN: Self = Self {
        privileged_nodes: true,
        private_network: true,
    };

    pub const MANAGED: Self = Self {
        privileged_nodes: false,
        private_network: false,
    };
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::OPEN
    }
}

#[must_use]
pub fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => is_public_v6(address),
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
        || first == 0
        || first >= 240
        || (first == 100 && (64..128).contains(&second))
        || (first == 192 && second == 0 && third == 0)
        || (first == 198 && (18..20).contains(&second)))
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_v4(mapped);
    }
    let segments = address.segments();
    let embedded = |high: u16, low: u16| {
        Ipv4Addr::new(
            (high >> 8) as u8,
            (high & 0xff) as u8,
            (low >> 8) as u8,
            (low & 0xff) as u8,
        )
    };
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_public_v4(embedded(segments[6], segments[7]));
    }
    if segments[0] == 0x2002 {
        return is_public_v4(embedded(segments[1], segments[2]));
    }
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

pub(crate) fn check_url(url: &Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "only http and https addresses are allowed, not {url}"
        ));
    }
    let host = url.host_str().ok_or_else(|| format!("{url} has no host"))?;
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    match literal.parse::<IpAddr>() {
        Ok(address) if !is_public(address) => Err(format!(
            "{url} points to a private or reserved address, which this server does not allow"
        )),
        _ => Ok(()),
    }
}

struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| is_public(address.ip()))
                .collect();
            if addresses.is_empty() {
                return Err(format!(
                    "{host} resolves only to private or reserved addresses, which this server does not allow"
                )
                .into());
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

fn follow(attempt: Attempt<'_>) -> reqwest::redirect::Action {
    if attempt.previous().len() >= MAX_REDIRECTS {
        return attempt.error("too many redirects");
    }
    match check_url(attempt.url()) {
        Ok(()) => attempt.follow(),
        Err(message) => attempt.error(message),
    }
}

pub(crate) fn http_client(sandbox: Sandbox) -> Result<Client, reqwest::Error> {
    let builder = Client::builder()
        .user_agent(concat!("parameters/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30));
    if sandbox.private_network {
        builder.build()
    } else {
        builder
            .dns_resolver(std::sync::Arc::new(PublicResolver))
            .redirect(Policy::custom(follow))
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_reserved_addresses_are_not_public() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "198.18.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "2001:db8::1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in [
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn literal_private_urls_are_refused() {
        let check = |url: &str| check_url(&url.parse().unwrap());
        assert!(check("http://169.254.169.254/latest/meta-data").is_err());
        assert!(check("http://[::1]:8080/").is_err());
        assert!(check("http://10.0.0.1").is_err());
        assert!(check("file:///etc/passwd").is_err());
        assert!(check("https://example.com/").is_ok());
        assert!(check("https://1.1.1.1/").is_ok());
    }
}
