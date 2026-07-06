pub trait BrowserPort: Send + Sync {
    fn open_url(&self, url: &str);
}
