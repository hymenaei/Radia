# Async, concurrency, and lifecycle tests

Test the async guarantees the code actually exposes. Use synchronization primitives, mock transports, paused clocks, or a deterministic state-machine runner to force the smallest interleaving that could violate a guarantee. Avoid arbitrary `sleep` calls.

## Candidate failure modes

Use only the rows that match the contract. They are examples, not a checklist.

| Area              | Meaningful scenarios when they are part of the contract                                                      |
| ----------------- | ------------------------------------------------------------------------------------------------------------- |
| Cancellation      | Cancel at a state where cleanup differs; check for leaked permits or stale effects                           |
| State transitions | A late event after replacement or disconnect must not resurrect stale state                                  |
| Backpressure      | A full queue or closed receiver has the specified bounded outcome                                           |
| Ordering          | Force an ordering that could change externally visible behavior                                              |
| Retry             | Check attempt caps, idempotency, or duplicate effects when promised                                          |
| Shutdown          | In-flight work drains or cancels as specified                                                                |
| Networking        | Partial I/O, framing, EOF, malformed input, timeouts, or duplicate responses when the boundary requires them |
| Locks             | Verify progress or lock ordering only when correctness depends on it                                          |

## Deterministic Tokio timers

`start_paused` requires Tokio's macros, runtime, time, and `test-util` features. Signal when the timed operation has started, check that it remains pending before the deadline, then let paused-time `sleep` advance the clock.

```rust
use std::{future::pending, time::Duration};
use tokio::sync::oneshot;
use tokio::time;

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
    time::sleep(Duration::from_secs(4)).await;
    assert!(!task.is_finished());
    time::sleep(Duration::from_secs(1)).await;
    assert!(task.await.expect("task must complete").is_err());
}
```

Replace the `pending` future with the client's operation and assert its timeout error and resource cleanup. Tokio's simulated time does not advance `std::time::Instant`.

For cancellation cleanup, hold a mock response, cancel the request, and verify cleanup through later behavior such as new work being admitted. Assert stale-response effects only when they are a realistic contract risk. Avoid private counters when observable behavior proves the same invariant.

## Race tests

- Use channels/oneshots/barriers to establish _happens-before_ points instead of assuming task scheduling order. Control the mock response path to force desired interleavings.
- A race reproduced only probabilistically can be a separate stress test; pair it with a deterministic state-machine or scheduler-controlled regression when possible.
- If testing atomics, memory ordering, or lock-free code, consider `loom` (which explores schedules); add Miri or sanitizers where appropriate. Not every async component benefits from loom.
- For real sockets use loopback and OS-assigned ports; avoid network access to third-party services in ordinary tests.
- Each test should own its temporary resources and clean up tasks, processes, channels, and files on completion. Beware test executors that spawn background tasks surviving the primary assertion.
- Avoid blindly increasing timeouts or enabling retries when a race test fails. Investigate dependency on scheduler, timing, global state, and incomplete shutdown.

## Assert absence when it is contractual

When the contract requires an action not to happen, make the assertion finite: canceled requests send no later packet, stale events leave the current generation unchanged, rejected oversized frames avoid the specified allocation, or shutdown leaves no in-flight tasks. Advance a fake clock, close the event source, or exhaust a bounded queue instead of waiting indefinitely for a missing event.
