use std::net::{Ipv4Addr, Ipv6Addr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    fn default_port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NotCanonicalHttpUrl;

#[derive(Clone, Debug)]
pub struct RequestUrl {
    scheme: Scheme,
    host: String,
    port: Option<u16>,
    path_for_request: String,
}

impl RequestUrl {
    pub fn parse(spec: &str) -> Result<Self, NotCanonicalHttpUrl> {
        let (scheme, rest) = if let Some(rest) = spec.strip_prefix("http://") {
            (Scheme::Http, rest)
        } else if let Some(rest) = spec.strip_prefix("https://") {
            (Scheme::Https, rest)
        } else {
            return Err(NotCanonicalHttpUrl);
        };
        let authority_end = rest.find(['/', '?', '#']).ok_or(NotCanonicalHttpUrl)?;
        if !rest[authority_end..].starts_with('/') {
            return Err(NotCanonicalHttpUrl);
        }
        let authority = &rest[..authority_end];
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, host_port)| host_port);
        let (host, port) = if host_port.starts_with('[') {
            let close = host_port.find(']').ok_or(NotCanonicalHttpUrl)?;
            match &host_port[close + 1..] {
                "" => (&host_port[..=close], None),
                port => (&host_port[..=close], Some(port.strip_prefix(':').ok_or(NotCanonicalHttpUrl)?)),
            }
        } else {
            match host_port.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (host_port, None),
            }
        };
        let port = match port {
            Some(port) => Some(port.parse::<u16>().map_err(|_| NotCanonicalHttpUrl)?),
            None => None,
        };
        if host.is_empty() || port == Some(scheme.default_port()) {
            return Err(NotCanonicalHttpUrl);
        }
        let path = &rest[authority_end..];
        let path_for_request = path.split_once('#').map_or(path, |(before, _)| before);
        Ok(RequestUrl { scheme, host: host.to_owned(), port, path_for_request: path_for_request.to_owned() })
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    pub fn host_and_optional_port(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        }
    }

    pub fn path_for_request(&self) -> &str {
        &self.path_for_request
    }

    pub fn is_cryptographic(&self) -> bool {
        self.scheme == Scheme::Https
    }

    pub fn uses_default_port(&self) -> bool {
        self.port.is_none()
    }

    pub fn is_localhost(&self) -> bool {
        let host = self.host.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(&self.host);
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return ip.octets()[0] == 127;
        }
        if let Ok(ip) = host.parse::<Ipv6Addr>() {
            return ip == Ipv6Addr::LOCALHOST;
        }
        let host = host.strip_suffix('.').unwrap_or(host).as_bytes();
        host.eq_ignore_ascii_case(b"localhost") || host.len() >= 10 && host[host.len() - 10..].eq_ignore_ascii_case(b".localhost")
    }
}
