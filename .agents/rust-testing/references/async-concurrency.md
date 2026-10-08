# Async, concurrency, and lifecycle tests

Async tests must assert externally observable ordering, completion, cancellation, progress, and bounded resources without relying on lucky scheduling. Use synchronization primitives, mock transports, paused clocks, or a deterministic state-machine runner rather than arbitrary `sleep` calls.

## Prioritize these cases when relevant

| Area              | Meaningful scenarios                                                                                                             |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Cancellation      | Before admission, after admission, after request enqueue, during response, during cleanup; no leaked permits or retained buffers |
| State transitions | Connect, replace, disconnect, reconnect, late event for old generation; no resurrection or dropped valid events                  |
| Backpressure      | Queue at capacity, slow consumer, closed receiver; bounded allocation and predictable rejection/await behavior                   |
| Ordering          | Events racing with lifecycle notifications; user-observable ordering invariant, not scheduler's exact poll order                 |
| Retry             | Transient and permanent errors; attempt cap, idempotency, no duplicate effects, correct reset                                    |
| Shutdown          | In-flight work drains or cancels as specified, tasks joined, channels closed, resources released                                 |
| Networking        | Partial reads/writes, fragmented frames, EOF, malformed payloads, timeouts, duplicate responses                                  |
| Locks             | No blocking operations on async executor; lock ordering and progress where correctness requires it                               |

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

## Cancellation example as a scenario (not copy-paste code)

1. Admit a request while the server-side mock deliberately holds its response.
2. Verify the permit is occupied (or that capacity-relevant public behavior reflects occupancy).
3. Cancel the initiating task and drive the runtime until cancellation cleanup has occurred.
4. Submit a new request and verify it is admitted, completes, and produces the correct response.
5. Verify old/canceled responses cannot mutate current state and buffered payloads are released.

Do not test permit accounting solely by reading a private counter when a later-admission behavior proves the contract; but checking a counter can be useful for debugging.

## Race tests

- Use channels/oneshots/barriers to establish _happens-before_ points instead of assuming task scheduling order. Control the mock response path to force desired interleavings.
- A race reproduced only probabilistically can be a separate stress test; pair it with a deterministic state-machine or scheduler-controlled regression when possible.
- If testing atomics, memory ordering, or lock-free code, consider `loom` (which explores schedules); add Miri or sanitizers where appropriate. Not every async component benefits from loom.
- For real sockets use loopback and OS-assigned ports; avoid network access to third-party services in ordinary tests.
- Each test should own its temporary resources and clean up tasks, processes, channels, and files on completion. Beware test executors that spawn background tasks surviving the primary assertion.
- Avoid blindly increasing timeouts or enabling retries when a race test fails. Investigate dependency on scheduler, timing, global state, and incomplete shutdown.

## Check the negative space

Tests should often confirm that _something does not happen_: canceled request never sends a second packet, stale event never changes the new generation, rejected oversized frame never allocates the advertised size, shutdown leaves no inflight tasks. Make such assertions finite: advance a fake clock, close the event source, or exhaust a well-defined queue; don't hang waiting for an event that may never arrive.
