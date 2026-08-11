//! Admission control for the PHP worker queue.
//!
//! The queue between the HTTP layer and the worker pool is bounded. Without
//! admission control a request that arrives while the queue is full is
//! rejected on the spot, so the shed threshold is the instantaneous queue
//! depth — a quantity derived from the worker count, not from how long the
//! request would actually have had to wait.
//!
//! `Admission` replaces that with a deadline. Three invariants carry it:
//!
//! - **Permit count equals queue capacity**, so holding a permit guarantees a
//!   free slot in the channel.
//! - **One deadline covers both waits.** A request waits twice — here for a
//!   slot, then in the channel for a worker — and the deadline is absolute, so
//!   the budget bounds their sum rather than the first alone. The second wait
//!   is enforced at pickup, where the request is refused instead of executed;
//!   this module only hands out the deadline.
//! - **The waiting set is capped.** A waiter holds its connection, so an
//!   uncapped set lets a sustained overload consume every connection permit
//!   until the accept loop stalls — answering overload by not answering.
//!
//! Kept out of `super::sapi` (which is gated behind the `php` feature) so the
//! policy is unit-testable on a host without `libphp.so`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

/// Why a request never reached a worker. The distinction is what an operator
/// needs to act on: the first three are overload and answer 529, the other two
/// are the pool going away, and counting those as overload is how an ordinary
/// restart comes to look like a traffic spike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShedReason {
    /// No wait budget configured — refused the moment the queue was full.
    QueueFull,
    /// The budget ran out before the request reached a worker — either waiting
    /// for a slot, or sitting in the queue once it had one.
    WaitTimeout,
    /// Too many requests were already waiting for a slot.
    WaitingFull,
    /// The gate is closed because the server is shutting down.
    ShuttingDown,
    /// No receiver is left on the queue: every worker thread is gone. Not
    /// backpressure — the pool is dead, and without a counter of its own that
    /// state is indistinguishable from the application returning 500.
    PoolUnavailable,
}

impl ShedReason {
    /// Every reason, in the order their counters are laid out.
    ///
    /// Both the counter slot and the label rendered next to it are derived
    /// from this array, so adding or reordering a variant can move a series
    /// but cannot leave one counting under another's name.
    pub const ALL: [ShedReason; 5] = [
        Self::QueueFull,
        Self::WaitTimeout,
        Self::WaitingFull,
        Self::ShuttingDown,
        Self::PoolUnavailable,
    ];

    /// Prometheus label value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QueueFull => "queue_full",
            Self::WaitTimeout => "wait_timeout",
            Self::WaitingFull => "waiting_full",
            Self::ShuttingDown => "shutting_down",
            Self::PoolUnavailable => "pool_unavailable",
        }
    }

    /// This reason's slot in a per-reason counter array — its position in
    /// [`ShedReason::ALL`].
    ///
    /// Searched rather than read off a `#[repr(usize)]` discriminant: the
    /// render pairs `ALL[i]` with counter `i`, so a discriminant would be a
    /// second ordering to keep in step with the array by hand.
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|r| *r == self)
            .expect("ShedReason::ALL is missing a variant")
    }
}

/// Outcome of waiting for admission.
pub enum Admitted {
    /// The request may enter the queue; the permit holds its slot.
    Slot(OwnedSemaphorePermit),
    Shed(ShedReason),
}

/// Queue admission gate: a permit per queue slot, the budget a request may
/// spend not executing, and a cap on how many may wait at once.
pub struct Admission {
    slots: Arc<Semaphore>,
    /// One permit per request allowed to park waiting for `slots`. Held only
    /// for the duration of the wait.
    parking: Arc<Semaphore>,
    /// `None` = fail fast: a request that finds no free slot is shed
    /// immediately rather than waiting.
    budget: Option<Duration>,
}

impl Admission {
    /// `capacity` must match the queue's capacity for the "a permit implies a
    /// free slot" invariant to hold. `wait_timeout_ms` of 0 means fail fast.
    /// `max_waiting` caps how many requests may be parked at once.
    ///
    /// `max_waiting` is deliberately *not* derived from `capacity`: how many
    /// requests can usefully wait is a function of the pool's service rate and
    /// the budget, not of how deep the queue is. Tying it to capacity would
    /// refuse burst absorption exactly where the queue is shallow, which is
    /// where absorbing bursts matters most.
    pub fn new(capacity: usize, wait_timeout_ms: u64, max_waiting: usize) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(capacity)),
            parking: Arc::new(Semaphore::new(max_waiting)),
            budget: (wait_timeout_ms > 0).then(|| Duration::from_millis(wait_timeout_ms)),
        }
    }

    /// How long a request may spend not executing, or `None` for fail-fast.
    ///
    /// The caller turns this into one absolute deadline at arrival and carries
    /// it through both waits, so a request cannot spend the budget twice.
    pub fn budget(&self) -> Option<Duration> {
        self.budget
    }

    /// Non-blocking claim on a queue slot — the hot path.
    ///
    /// Reports *why* it failed rather than just that it did: a full queue is
    /// backpressure, a closed gate is an instance going away, and answering a
    /// client "overloaded, retry in 3" during teardown tells it the wrong
    /// thing.
    pub fn try_admit(&self) -> Result<OwnedSemaphorePermit, ShedReason> {
        Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|e| match e {
                TryAcquireError::Closed => ShedReason::ShuttingDown,
                TryAcquireError::NoPermits => ShedReason::QueueFull,
            })
    }

    /// Claim a place in the waiting set, before committing to a wait.
    ///
    /// Synchronous so that a refusal past the cap — the steady state of a
    /// sustained overload, and so the shed an overloaded server emits most —
    /// costs neither a wait nor the boxed future one requires.
    pub fn try_park(&self) -> Result<OwnedSemaphorePermit, ShedReason> {
        Arc::clone(&self.parking)
            .try_acquire_owned()
            .map_err(|e| match e {
                TryAcquireError::Closed => ShedReason::ShuttingDown,
                TryAcquireError::NoPermits => ShedReason::WaitingFull,
            })
    }

    /// Refuse all further admissions and wake every waiter with a shed.
    ///
    /// Called on executor teardown: a request parked on `admit` holds a clone
    /// of the queue's sender, which would keep the channel open and leave the
    /// worker threads blocked in `recv` forever.
    pub fn close(&self) {
        self.slots.close();
        self.parking.close();
    }

    /// Wait for a queue slot until `deadline`, holding the place claimed by
    /// [`Admission::try_park`].
    ///
    /// Tokio's semaphore hands out permits in FIFO order, so waiters are
    /// admitted in arrival order and none can starve.
    ///
    /// A client that goes away mid-wait needs no handling here: the connection
    /// layer drops the request future, this future goes with it, and the place
    /// is released on the spot — sooner than any signal could be delivered.
    /// That covers a client that closes, on either protocol, under the
    /// preconditions `handle_request` documents. A client that stops waiting
    /// without closing is invisible to every layer, and does hold its place
    /// until it is admitted or the budget runs out.
    pub async fn admit(&self, parked: OwnedSemaphorePermit, deadline: Instant) -> Admitted {
        // A slot can free between the caller's fast-path attempt and this
        // future's first poll — the two are separated by a return through the
        // connection layer. Without this retry such a request parks behind a
        // queue it could have walked into.
        if let Ok(permit) = self.try_admit() {
            return Admitted::Slot(permit);
        }

        let slots = Arc::clone(&self.slots);
        let outcome = match tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            slots.acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => Admitted::Slot(permit),
            // The gate was closed under us — teardown, not backpressure.
            Ok(Err(_)) => Admitted::Shed(ShedReason::ShuttingDown),
            Err(_) => Admitted::Shed(ShedReason::WaitTimeout),
        };
        drop(parked);
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the executor does on a missed fast path: claim a place, then wait
    /// for a slot until the budget runs out.
    async fn park_and_admit(admission: &Admission) -> Admitted {
        let budget = admission
            .budget()
            .expect("no budget — the caller fails fast instead of waiting");
        match admission.try_park() {
            Ok(parked) => admission.admit(parked, Instant::now() + budget).await,
            Err(reason) => Admitted::Shed(reason),
        }
    }

    #[test]
    fn try_admit_hands_out_exactly_capacity_permits() {
        let admission = Admission::new(2, 1000, 64);
        let a = admission.try_admit();
        let b = admission.try_admit();
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(
            admission.try_admit().err(),
            Some(ShedReason::QueueFull),
            "third claim must fail — capacity is 2"
        );

        // Releasing one frees exactly one slot. Bind the replacement — an
        // unbound `try_admit()` inside `assert!` drops its permit on the spot
        // and would hand the slot straight back.
        drop(a);
        let c = admission.try_admit();
        assert!(c.is_ok());
        assert!(admission.try_admit().is_err());
        drop((b, c));
    }

    #[tokio::test]
    async fn admit_sheds_once_the_budget_expires() {
        let admission = Admission::new(1, 250, 64);
        let _held = admission.try_admit().expect("first claim succeeds");

        let start = std::time::Instant::now();
        assert!(
            matches!(
                park_and_admit(&admission).await,
                Admitted::Shed(ShedReason::WaitTimeout)
            ),
            "no slot ever freed — must shed, and say the budget is why"
        );
        assert!(
            start.elapsed() >= Duration::from_millis(250),
            "must wait the full budget before shedding, waited {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn admit_takes_a_slot_freed_mid_wait() {
        let admission = Admission::new(1, 30_000, 64);
        let held = admission.try_admit().expect("first claim succeeds");

        let releaser = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            drop(held);
        });

        assert!(
            matches!(park_and_admit(&admission).await, Admitted::Slot(_)),
            "slot freed well inside the budget — must be admitted"
        );
        releaser.await.unwrap();
    }

    #[tokio::test]
    async fn close_wakes_waiters_instead_of_holding_them_to_the_budget() {
        // Teardown must not leave a waiter parked for its whole budget — it
        // would hold a sender clone and keep the workers' channel open.
        let admission = Arc::new(Admission::new(1, 30_000, 64));
        let _held = admission.try_admit().expect("first claim succeeds");

        let closer = {
            let admission = Arc::clone(&admission);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                admission.close();
            })
        };

        let start = std::time::Instant::now();
        assert!(
            matches!(
                park_and_admit(&admission).await,
                Admitted::Shed(ShedReason::ShuttingDown)
            ),
            "a closed gate must shed as teardown, not as backpressure"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "close must wake the waiter, not wait out the budget"
        );
        closer.await.unwrap();
    }

    #[tokio::test]
    async fn parking_cap_sheds_instead_of_growing_the_waiting_set() {
        // One queue slot, one parking spot. With the slot held and the spot
        // taken by a parked waiter, a third arrival must be shed on the spot
        // rather than join the queue of waiters.
        let admission = Arc::new(Admission::new(1, 30_000, 1));
        let _held = admission.try_admit().expect("first claim succeeds");

        let parked = {
            let admission = Arc::clone(&admission);
            tokio::spawn(async move { park_and_admit(&admission).await })
        };
        // Let the spawned waiter reach `admit` and take the parking permit.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Synchronous on purpose: this is the refusal a sustained overload
        // emits most, so it must not cost the caller a future to find out.
        assert_eq!(
            admission.try_park().err(),
            Some(ShedReason::WaitingFull),
            "waiting set is full — must refuse, and say the cap is why"
        );
        assert!(
            matches!(
                park_and_admit(&admission).await,
                Admitted::Shed(ShedReason::WaitingFull)
            ),
            "and the whole path must answer the same"
        );

        admission.close();
        let _ = parked.await;
    }

    #[tokio::test]
    async fn a_slot_freed_before_the_first_poll_is_taken_not_waited_for() {
        // The fast-path attempt and this future's first poll are separated by
        // a return through the connection layer, so a slot can free in
        // between. An already-expired deadline makes the retry the only thing
        // that can produce a permit here.
        let admission = Admission::new(1, 1000, 1);
        let held = admission.try_admit().expect("first claim succeeds");
        let parked = admission.try_park().expect("the set is empty");
        drop(held);

        assert!(
            matches!(
                admission.admit(parked, Instant::now()).await,
                Admitted::Slot(_)
            ),
            "a free slot must be taken before the deadline is consulted"
        );
    }

    #[tokio::test]
    async fn parking_permit_is_released_after_the_wait() {
        // The cap bounds concurrent waiters, not total waits: a shed waiter
        // must free its parking spot for the next arrival, or the gate
        // ratchets shut after `max_waiting` sheds.
        let admission = Admission::new(1, 60, 1);
        let _held = admission.try_admit().expect("first claim succeeds");

        for attempt in 0..3 {
            let start = std::time::Instant::now();
            assert!(
                matches!(park_and_admit(&admission).await, Admitted::Shed(_)),
                "attempt {attempt}"
            );
            assert!(
                start.elapsed() >= Duration::from_millis(60),
                "attempt {attempt} returned in {:?} — the parking permit leaked \
                 and this arrival was refused instead of waiting",
                start.elapsed()
            );
        }
    }

    #[test]
    fn fail_fast_still_tells_teardown_from_backpressure() {
        // With no budget the gate never parks, so `try_admit` is the only
        // thing that sees the closure. If it collapses both failures into one,
        // a request arriving during shutdown is answered "overloaded, retry in
        // 3" — the client is told to come back to an instance that is leaving,
        // and the shed is counted as backpressure that never happened.
        let admission = Admission::new(1, 0, 64);
        admission.close();

        assert_eq!(
            admission.try_admit().err(),
            Some(ShedReason::ShuttingDown),
            "a closed gate is teardown, not a full queue"
        );
        assert_eq!(
            admission.try_park().err(),
            Some(ShedReason::ShuttingDown),
            "and the waiting set must not read a closed gate as its own cap"
        );
    }

    #[test]
    fn closed_gate_reports_teardown_even_with_slots_free() {
        // The distinction cannot come from "were there permits left": here
        // every permit is free and the answer must still be ShuttingDown.
        let admission = Admission::new(8, 1000, 64);
        admission.close();
        assert_eq!(
            admission.try_admit().err(),
            Some(ShedReason::ShuttingDown),
            "capacity was untouched — only the closure can explain this refusal"
        );
    }

    #[test]
    fn zero_budget_reports_no_deadline_to_wait_against() {
        // `budget()` is what the caller branches on to stay off the wait path
        // entirely; `None` is the fail-fast contract.
        let admission = Admission::new(1, 0, 64);
        assert!(admission.budget().is_none());
        let _held = admission.try_admit().expect("first claim succeeds");
        assert_eq!(admission.try_admit().err(), Some(ShedReason::QueueFull));
    }

    #[tokio::test]
    async fn a_dropped_wait_gives_its_place_back() {
        // A client that goes away is handled by nothing in this module: the
        // connection layer drops the request future, which drops the wait,
        // which releases the parking permit. This is what keeps a balancer's
        // timed-out retries from holding a cap they are no longer using, so it
        // is worth a test even though no code here implements it.
        let admission = Admission::new(1, 30_000, 1);
        let _held = admission.try_admit().expect("first claim succeeds");

        // An outer timeout drops the wait mid-flight, exactly as the
        // connection layer does.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), park_and_admit(&admission))
                .await
                .is_err(),
            "the wait must still have been parked when it was dropped"
        );

        // The only parking spot must be free again. A `waiting_full` would
        // resolve immediately and the outer timeout would return `Ok`.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), park_and_admit(&admission))
                .await
                .is_err(),
            "the next arrival was refused for a cap the departed waiter had left"
        );
    }

    #[test]
    fn every_reason_has_its_own_slot_and_label() {
        // Adding a variant without listing it in ALL fails to compile here:
        // the match is exhaustive and has no wildcard arm.
        for reason in ShedReason::ALL {
            match reason {
                ShedReason::QueueFull
                | ShedReason::WaitTimeout
                | ShedReason::WaitingFull
                | ShedReason::ShuttingDown
                | ShedReason::PoolUnavailable => {}
            }
            assert_eq!(
                ShedReason::ALL[reason.index()],
                reason,
                "{} does not round-trip through its counter slot",
                reason.as_str()
            );
        }

        let mut labels: Vec<&str> = ShedReason::ALL.iter().map(|r| r.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(
            labels.len(),
            ShedReason::ALL.len(),
            "two reasons render as the same series and would sum into one"
        );
    }
}
