use std::fmt;

use url::{Host, Url};

const MESSAGE_LINK_SCHEMES: &[&str] = &["http", "https", "mailto"];
const LOOPBACK_NAME: &str = "localhost";

#[derive(Debug, Clone)]
pub struct LauncherSafeUrl(Url);

impl LauncherSafeUrl {
    pub fn message_link(raw: impl AsRef<str>) -> Option<Self> {
        let link = Url::parse(raw.as_ref()).ok()?;
        MESSAGE_LINK_SCHEMES
            .contains(&link.scheme())
            .then_some(Self(link))
    }

    pub fn sign_in_page(page: Url) -> Result<Self, InsecureSignInPage> {
        match page.scheme() {
            "https" => Ok(Self(page)),
            "http" if is_loopback(&page) => Ok(Self(page)),
            _ => Err(InsecureSignInPage(page)),
        }
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

fn is_loopback(page: &Url) -> bool {
    match page.host() {
        Some(Host::Domain(name)) => name == LOOPBACK_NAME,
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[derive(Debug)]
pub struct InsecureSignInPage(Url);

impl fmt::Display for InsecureSignInPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let page = &self.0;
        match page.host_str() {
            Some(host) => write!(f, "the sign-in page is {}://{host}", page.scheme()),
            None => write!(f, "the sign-in page is a {}: URL", page.scheme()),
        }
    }
}
