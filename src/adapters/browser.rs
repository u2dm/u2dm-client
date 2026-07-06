use crate::ports::browser::BrowserPort;

pub struct DesktopBrowser;

impl DesktopBrowser {
    pub fn new() -> Self {
        Self
    }
}

impl BrowserPort for DesktopBrowser {
    fn open_url(&self, url: &str) {
        open::that_in_background(url);
    }
}
