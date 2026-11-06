use tokio::sync::{mpsc, watch};

#[derive(Default)]
pub struct Revisions(watch::Sender<u64>);

impl Revisions {
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }

    pub fn bump(&self) {
        self.0
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

pub async fn report_changes<T: PartialEq + Clone + Send>(
    mut revision: watch::Receiver<u64>,
    tx: &mpsc::Sender<T>,
    read: impl Fn() -> T + Send,
) {
    let mut reported = None;
    loop {
        let value = read();
        if reported.as_ref() != Some(&value) {
            if tx.send(value.clone()).await.is_err() {
                return;
            }
            reported = Some(value);
        }
        if revision.changed().await.is_err() {
            return;
        }
    }
}
