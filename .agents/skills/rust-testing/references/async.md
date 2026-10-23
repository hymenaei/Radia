# Async and concurrency

Force the smallest interleaving that could violate a guarantee, using channels, oneshots, barriers, mock transports, paused clocks, or a deterministic runner. Time passes only through the clock.

## Scenarios

Pick those the contract implies:

- Cancellation at states where cleanup differs
- Late events after replacement or disconnect
- Full queue or closed receiver
- Visible ordering
- Retry caps and idempotency
- Shutdown drain or cancel
- Partial I/O, EOF, malformed input, duplicate responses
- Lock ordering

## Example

```rust
use std::{future::pending, time::Duration};
use tokio::{sync::oneshot, time};

#[tokio::test(start_paused = true)]
async fn request_times_out_at_deadline() {
    let (started_tx, started_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        time::timeout(Duration::from_secs(5), async {
            started_tx.send(()).expect("test receiver is alive");
            pending::<()>().await
        })
        .await
    });

    started_rx.await.expect("request started");
    time::advance(Duration::from_secs(4)).await;
    assert!(!task.is_finished());
    time::advance(Duration::from_secs(1)).await;
    assert!(task.await.expect("task must complete").is_err());
}
```

`start_paused` needs Tokio's `macros`, `rt`, `time`, and `test-util` features and does not advance `std::time::Instant`. Swap `pending` for the real operation and assert the timeout error and cleanup.

## Rules

- Verify cancellation cleanup through later behavior (new work is admitted), not private counters.
- A race reproducible only probabilistically gets a separate stress test plus a deterministic regression. Use `loom` for atomics and lock-free code.
- Real sockets bind loopback on OS-assigned ports. Each test owns and cleans up its tasks, processes, channels, and files.
- Asserting absence (nothing sent, state unchanged, no task in flight) needs a finite bound. Advance a fake clock, close the event source, or exhaust a bounded queue.
