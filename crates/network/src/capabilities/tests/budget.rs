use std::sync::{Arc, mpsc};

use tokio::{
    spawn,
    sync::{Semaphore, oneshot},
};

use super::spawn_blocking_with_permit;

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_keeps_permit_until_blocking_work_finishes() {
    let slots = Arc::new(Semaphore::new(1));
    let permit = Arc::clone(&slots).acquire_owned().await.expect("request slot");
    let (entered, entered_rx) = oneshot::channel();
    let (release, release_rx) = mpsc::channel();
    let task = spawn(spawn_blocking_with_permit(permit, move || {
        entered.send(()).expect("blocking work receiver is alive");
        release_rx.recv().expect("release blocking work");
    }));

    entered_rx.await.expect("blocking work started");
    task.abort();
    assert!(task.await.expect_err("waiter must be canceled").is_cancelled());
    assert_eq!(slots.available_permits(), 0, "cancellation cannot release work admission early");

    release.send(()).expect("unblock running work");
    let permit = slots.acquire().await.expect("blocking work completion releases its permit");
    drop(permit);
}
