use std::env;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

const BUNDLED_CATALOGS: &str = env!("U2DM_BUNDLED_CATALOGS");
#[cfg(unix)]
const DOMAIN: &str = env!("CARGO_PKG_NAME");
#[cfg(unix)]
const CATALOG_ROOT: &str = "locale";

pub const MESSAGE_LOCALE_KEYS: &[&str] = &["LC_ALL", "LC_MESSAGES", "LANG"];
pub const TIME_LOCALE_KEYS: &[&str] = &["LC_ALL", "LC_TIME", "LANG"];

pub struct LocaleRequest {
    name: String,
    modifier: Option<String>,
}

impl LocaleRequest {
    pub fn from_env(keys: &[&str]) -> Option<Self> {
        let requested = keys.iter().find_map(|key| non_empty_var(key))?;
        Self::parse(&requested)
    }

    pub fn parse(tag: &str) -> Option<Self> {
        let (head, modifier) = match tag.split_once('@') {
            Some((head, modifier)) => (head, Some(modifier)),
            None => (tag, None),
        };
        let name = head
            .split_once('.')
            .map_or(head, |(name, _)| name)
            .replace('-', "_");
        if name.is_empty() {
            return None;
        }

        Some(Self {
            name,
            modifier: modifier.filter(|m| !m.is_empty()).map(str::to_owned),
        })
    }

    pub fn is_untranslated(&self) -> bool {
        self.name == "C" || self.name == "POSIX"
    }

    pub fn candidates(&self) -> Vec<String> {
        let mut candidates = Vec::new();
        let mut base = self.name.as_str();
        loop {
            if let Some(modifier) = &self.modifier {
                candidates.push(format!("{base}@{modifier}"));
            }
            candidates.push(base.to_owned());

            let Some(shorter) = base.rfind('_').and_then(|cut| base.get(..cut)) else {
                return candidates;
            };
            base = shorter;
        }
    }
}

pub fn catalog_dir() -> PathBuf {
    let locales = LocaleRequest::from_env(MESSAGE_LOCALE_KEYS)
        .filter(|request| !request.is_untranslated())
        .map_or_else(Vec::new, |request| request.candidates());

    installed_catalog_dir(&locales).unwrap_or_else(|| PathBuf::from(BUNDLED_CATALOGS))
}

#[cfg(unix)]
fn installed_catalog_dir(locales: &[String]) -> Option<PathBuf> {
    let base = xdg::BaseDirectories::new();
    base.get_data_home()
        .into_iter()
        .chain(base.get_data_dirs())
        .map(|data_dir| data_dir.join(CATALOG_ROOT))
        .find(|dir| locales.iter().any(|locale| has_catalog(dir, locale)))
}

#[cfg(not(unix))]
fn installed_catalog_dir(_locales: &[String]) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn has_catalog(dir: &Path, locale: &str) -> bool {
    dir.join(locale)
        .join("LC_MESSAGES")
        .join(format!("{DOMAIN}.mo"))
        .is_file()
}

fn non_empty_var(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.is_empty())
}
