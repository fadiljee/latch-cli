use crate::db::{Db, StoreError};
use std::path::Path;
use std::thread;
use tokio::sync::{mpsc, oneshot};

type Job = Box<dyn FnOnce(&mut Db) + Send>;

const STORE_QUEUE_CAP: usize = 256;

/// Handle async ke aktor penulis tunggal SQLite yang berjalan di OS thread terpisah.
#[derive(Clone)]
pub struct Store {
    tx: mpsc::Sender<Job>,
}

impl Store {
    /// Membuka database di thread aktor khusus dan mengembalikan handle `Store`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let mut db = Db::open(path)?;
        let (tx, mut rx) = mpsc::channel::<Job>(STORE_QUEUE_CAP);

        thread::Builder::new()
            .name("latch-store".to_string())
            .spawn(move || {
                while let Some(job) = rx.blocking_recv() {
                    job(&mut db);
                }
            })?;

        Ok(Self { tx })
    }

    /// Menjalankan operasi sinkron pada `Db` di thread aktor tanpa memblokir runtime Tokio.
    pub async fn call<F, T>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Db) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let (resp_tx, resp_rx) = oneshot::channel();
        let job: Job = Box::new(move |db| {
            let res = f(db);
            let _ = resp_tx.send(res);
        });
        self.tx
            .send(job)
            .await
            .map_err(|_| StoreError::ActorClosed)?;
        resp_rx.await.map_err(|_| StoreError::ActorClosed)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn actor_executes_queries_off_async_runtime() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("actor.db")).unwrap();
        let projects = store.call(|db| db.projects_list()).await.unwrap();
        assert!(projects.is_empty());
    }
}
