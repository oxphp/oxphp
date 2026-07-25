/* ext/oxphp_fiber.h — Fiber-based request multiplexing scheduler. */
#ifndef OXPHP_FIBER_H
#define OXPHP_FIBER_H

#include "php.h"
#include "zend_fibers.h"

#include <poll.h>

/* Maximum concurrent fibers per worker thread. */
#define OXPHP_MAX_FIBERS 256

/* Maximum descriptors one fiber may wait on in a single suspension. Bounds the
 * stack array every caller of oxphp_fiber_io_wait() builds, and the width of
 * the aggregated poll set the scheduler assembles from all parked fibers. */
#define OXPHP_MAX_WAIT_FDS 64

/* ─── Suspend Reasons ──────────────────────────────────── */

typedef enum {
    OXPHP_SUSPEND_NONE = 0,
    OXPHP_SUSPEND_AWAIT,        /* waiting for oxphp_async_await result */
    OXPHP_SUSPEND_AWAIT_ALL,    /* waiting for oxphp_async_await_all */
    OXPHP_SUSPEND_AWAIT_ANY,    /* waiting for oxphp_async_await_any */
    OXPHP_SUSPEND_SLEEP,        /* waiting for oxphp_sleep timer */
    OXPHP_SUSPEND_IO_WAIT,      /* waiting for readiness of a file descriptor */
} oxphp_suspend_reason;

/* ─── Per-Fiber Saved PHP State ────────────────────────── */

/* ─── Per-Fiber VM State ───────────────────────────────── */

/* Replicates zend_fiber_vm_state from zend_fibers.c (internal/static).
 * The low-level fiber API (zend_fiber_switch_context) does NOT save VM state —
 * only the high-level API (zend_fiber_start/resume) does. We must do it
 * ourselves for each context switch. Field set mirrors upstream; the
 * ZEND_CHECK_STACK_LIMIT stack_base/stack_limit pair is the only exception —
 * we handle those separately via fiber->saved_stack_* (estimated from the C
 * stack pointer at coroutine entry, since the fiber stack struct is opaque). */
typedef struct {
    zend_vm_stack vm_stack;
    zval *vm_stack_top;
    zval *vm_stack_end;
    size_t vm_stack_page_size;
    zend_execute_data *current_execute_data;
    int error_reporting;
    /* JIT tracing: nonzero while the tracing JIT is recording a trace. Must be
     * saved/restored per fiber so a fiber that suspends mid-trace does not leak
     * its in-progress trace number into another fiber's run (trace corruption). */
    uint32_t jit_trace_num;
    JMP_BUF *bailout;
    zend_fiber *active_fiber;
} oxphp_fiber_vm_state;

/* ─── Per-Fiber PHP State ──────────────────────────────── */

typedef struct {
    /* Superglobals: saved PG(http_globals) values */
    zval http_globals[6]; /* TRACK_VARS_POST .. TRACK_VARS_FILES */

    /* SAPI state */
    zend_llist sapi_headers;
    int http_response_code;
    bool headers_sent;

    /* PG(connection_status) is thread-global; without per-fiber save/restore
     * one fiber's PHP_CONNECTION_ABORTED (drain bail, client abort) would leak
     * into every other multiplexed fiber on the same worker thread. */
    int connection_status;

    /* The bridge's per-request ctx flags live in its __thread ctx and are
     * wiped by every new request's prep (oxphp_bridge_reset_request_ctx), so
     * under multiplexing a suspended fiber's response state is gone by the
     * time it resumes. All three are monotonic within a request (they only
     * ever go false→true), so capturing at suspend and re-installing at resume
     * is a faithful restore. Both C (oxphp_stream_flush, oxphp_finish_request)
     * and Rust (oxphp_flush, ub_write, try_early_send, the drain flush guard)
     * read them, so the whole trio has to travel with the fiber. */
    bool bridge_stream_mode;    /* ctx.stream_mode: chunked output has begun */
    bool bridge_headers_sent;   /* ctx.headers_sent: stream headers on the wire */
    bool bridge_finished;       /* ctx.finished: oxphp_finish_request() was called */

    /* Parked unhandled-exception capture (opaque oxphp_unhandled_slot*, owned).
     * The capture is taken in the coroutine's catch arm (oxphp_capture_unhandled,
     * from the live EG(exception)) after the handler returns, before shutdown
     * functions run; if a shutdown function suspends, the bridge's thread-active
     * capture slot is moved here (take) so the next request's reset can't wipe
     * it, and moved back (restore) when this fiber resumes. NULL whenever the
     * fiber holds no parked capture (the common case). */
    void *unhandled_exc;

    /* Zend VM state (vm_stack, execute_data, bailout, etc.) */
    oxphp_fiber_vm_state vm_state;

    /* Output buffer: we flush OB to ub_write on suspend,
     * so no OB state needs saving. The output is in Rust RESPONSE TLS. */
} oxphp_fiber_php_state;

/* ─── Request Fiber ────────────────────────────────────── */

typedef struct _oxphp_request_fiber {
    /* Low-level fiber context (VM state saved automatically on switch) */
    zend_fiber_context context;

    /* Pointer back to scheduler context for suspend */
    zend_fiber_context *scheduler;

    /* Unique ID for this fiber (used as key for Rust TLS slot management) */
    uint64_t fiber_id;

    /* Saved PHP state (superglobals, SAPI headers) */
    oxphp_fiber_php_state php_state;

    /* Suspend reason and associated data */
    oxphp_suspend_reason suspend_reason;
    union {
        int64_t promise_id;           /* AWAIT: single promise */
        struct {                      /* AWAIT_ALL: multiple promises */
            int64_t *ids;
            uint32_t count;
            uint32_t completed;
        } await_all;
        struct {                      /* AWAIT_ANY: multiple promises */
            int64_t *ids;
            uint32_t count;
        } await_any;
        uint64_t timer_id;            /* SLEEP: timer ID */
        struct {                      /* IO_WAIT: descriptor readiness */
            /* Borrowed, NOT owned: points into the C stack frame of the fiber
             * parked here. That frame stays alive for exactly as long as the
             * fiber is suspended — a fiber only leaves this suspension by
             * returning from zend_fiber_switch_context() inside the very frame
             * that filled this in — so the pointer is valid for the whole wait
             * and there is nothing to free. Do not make it outlive the wait. */
            struct pollfd *fds;
            uint32_t nfds;
            bool expired;             /* deadline elapsed before any fd was ready */
            uint64_t deadline_ns;     /* CLOCK_MONOTONIC deadline, 0 = wait forever */
        } io;
    } suspend_data;

    /* The zend_fcall_info/cache for the handler closure (shared, not owned) */
    zend_fcall_info *fci;
    zend_fcall_info_cache *fcc;

    /* Stack limits for this fiber's C stack (set in coroutine entry,
     * saved on suspend, restored on resume). Needed because the low-level
     * fiber API doesn't manage EG(stack_base/limit). */
    void *saved_stack_base;
    void *saved_stack_limit;

    /* Handler result tracking */
    bool handler_failed;
    bool completed;          /* set by coroutine before final switch — low-level API never sets DEAD */
    bool started;            /* true after first start — reused fibers skip zend_fiber_init_context */
    int consecutive_errors;

    /* ── Async-task mode (oxphp_async fiber, not an HTTP request) ──
     * When task_mode is true the fiber runs a single per-task closure and
     * captures its result instead of producing an HTTP response. The closure
     * and call-info are owned by the fiber and torn down at finalize; args /
     * op_array / static_vars are borrowed (the Rust driver frees them once the
     * completed result has been drained). */
    bool task_mode;
    bool cancel_requested;          /* set cross-path; checked at suspend points + interrupt handler */
    bool timed_out;                 /* set by the scheduler when the awaited promise's per-call
                                     * timeout elapses while suspended; checked at the await suspend
                                     * point, which unwinds the await as a timeout instead of a result */
    uint64_t await_deadline_ns;     /* CLOCK_MONOTONIC deadline for the current AWAIT suspend.
                                     * 0 = no deadline (timeout <= 0, i.e. wait forever). */
    _Atomic(uint8_t) *cancel_cell;  /* borrowed: &CancelShared.cancelled (Rust AtomicBool).
                                     * The awaiter sets it cross-thread and kicks vm_interrupt; the
                                     * interrupt handler reads it to unwind a CPU-bound task fiber.
                                     * NULL for HTTP request fibers. */
    _Atomic(uint8_t) *request_cancel_ptr; /* borrowed: this HTTP request's CancelReason cell
                                     * (Rust CancellationState). Captured in
                                     * oxphp_scheduler_create_fiber (both request paths run the
                                     * Rust prep, which installs the cell, immediately before) and
                                     * re-installed into the bridge ctx on every resume, so the
                                     * interrupt handler and suspend points read THIS fiber's reason
                                     * rather than whichever request last set the per-thread ptr —
                                     * the fix for cancellation under fiber multiplexing. NULL for
                                     * task-mode fibers. */
    bool drain_kill;                /* set by the scheduler's drain sweep when it force-resumes
                                     * this suspended fiber; the suspend point unwinds uncatchably
                                     * on it. Deliberately independent of request_cancel_ptr: the
                                     * cell may already hold ClientAbort/Timeout, and the resume
                                     * must still bail instead of falling through to a blocking
                                     * await on an unsettled promise. */
    /* The drain sweep's soft phase kills only fibers with an OPEN stream —
     * php_state.bridge_stream_mode && !php_state.bridge_finished, captured at
     * the last suspend. An ordinary request suspended in a short await/sleep,
     * and a streaming request that already completed its response via
     * oxphp_finish_request(), both get the whole drain window; the deadline
     * (hard) phase kills every suspended fiber regardless. */
    zval task_closure;              /* owned: dtor at finalize */
    zend_fcall_info task_fci;
    zend_fcall_info_cache task_fcc;
    zval *task_args;                /* borrowed (Rust-owned) */
    uint32_t task_argc;
    zval task_retval;               /* ZVAL_UNDEF until completed */
    char *task_exc_class;           /* malloc'd, NULL if none */
    char *task_exc_message;         /* malloc'd, NULL if none */

    /* Linked list pointers for the scheduler's fiber list */
    struct _oxphp_request_fiber *next;
    struct _oxphp_request_fiber *prev;
} oxphp_request_fiber;

/* ─── Fiber Scheduler ──────────────────────────────────── */

/* Which fiber, and which of its descriptors, an entry of the aggregated poll
 * set came from — so readiness can be scattered back into each fiber's own
 * array, where the waiting code reads it. */
struct oxphp_io_owner {
    struct _oxphp_request_fiber *fiber;
    uint32_t idx;
};

typedef struct {
    /* The scheduler's own context (main thread context) */
    zend_fiber_context main_context;

    /* Doubly-linked list of active fibers */
    oxphp_request_fiber *fibers_head;
    oxphp_request_fiber *fibers_tail;
    uint32_t fiber_count;

    /* Pool of pre-allocated fiber structs to avoid malloc per request */
    oxphp_request_fiber *free_list;

    /* Currently running fiber (NULL when in scheduler) */
    oxphp_request_fiber *current;

    /* Fiber ID counter */
    uint64_t next_fiber_id;

    /* Shared handler closure (passed to all fibers) */
    zend_fcall_info *shared_fci;
    zend_fcall_info_cache *shared_fcc;

    /* Error tracking across fibers (mirrors the outer loop's consecutive_errors) */
    int consecutive_errors;
    uint64_t total_requests_done;

    /* Aggregated poll set, grown on demand and reused across ticks. Owned by
     * the scheduler because with several descriptors per fiber the worst case
     * (OXPHP_MAX_FIBERS * OXPHP_MAX_WAIT_FDS) is far too large for a stack
     * frame. Freed by oxphp_scheduler_destroy(). */
    struct pollfd *io_fds;
    struct oxphp_io_owner *io_owners;
    uint32_t io_cap;
} oxphp_fiber_scheduler;

/* ─── TLS: current fiber pointer ───────────────────────── */

/* Set when a fiber is executing, NULL in scheduler context.
 * Used by oxphp_async_await/oxphp_sleep to detect fiber mode. */
extern __thread oxphp_request_fiber *oxphp_current_fiber;

/* fiber_id of the currently executing request fiber, 0 outside fiber context.
 * Registered into the bridge at MINIT (oxphp_bridge_set_current_fiber_id_fn)
 * so Rust can tag async promise ownership at creation. */
uint64_t oxphp_fiber_current_id(void);

/* ─── Scheduler API ────────────────────────────────────── */

void oxphp_scheduler_init(oxphp_fiber_scheduler *sched);
void oxphp_scheduler_destroy(oxphp_fiber_scheduler *sched);

oxphp_request_fiber *oxphp_scheduler_create_fiber(
    oxphp_fiber_scheduler *sched,
    zend_fcall_info *fci,
    zend_fcall_info_cache *fcc
);

void oxphp_scheduler_start_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber);
void oxphp_scheduler_resume_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber, zval *value);
void oxphp_scheduler_finalize_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber);

/* Run one tick of the event loop: check try_recv, timers, await results. */
int oxphp_scheduler_tick(oxphp_fiber_scheduler *sched);

/* Idle backoff that waits on the descriptors parked fibers are blocked on,
 * rather than sleeping blind for the same interval. Returns false when there
 * was nothing to wait on, or when the wait itself failed — either way the
 * caller must fall back to its own sleep, or the loop would spin. */
bool oxphp_scheduler_io_backoff(oxphp_fiber_scheduler *sched, int64_t ns);

/* Same, for this thread's async-task scheduler. Returns false when nothing is
 * parked on a descriptor (or no task scheduler exists yet). */
bool oxphp_async_sched_io_backoff(int64_t ns);

/* Save/restore PHP state around context switches. */
void oxphp_fiber_save_php_state(oxphp_request_fiber *fiber);
void oxphp_fiber_restore_php_state(oxphp_request_fiber *fiber);

/* Targeted per-fiber request init (safe to call while other fibers are suspended).
 * Unlike oxphp_soft_reset(), this does NOT touch global OB or other thread-wide state.
 * It only initializes fresh superglobals and SAPI headers for the new request. */
void oxphp_fiber_init_request_state(void);

/* ─── Async-task scheduler (Rust-driven via bridge callbacks) ───
 * Registered into the bridge at MINIT via
 * oxphp_bridge_set_async_sched_callbacks. See ext/bridge/oxphp_bridge.h
 * for the contract. Stub bodies land first; the real per-thread task
 * scheduler fills them in. */
int64_t oxphp_async_sched_spawn(void *op_array, void *static_vars,
                                void *this_ptr, uint32_t argc, void *args,
                                void *cancel_cell);
int     oxphp_async_sched_tick(void);
int64_t oxphp_async_sched_poll_completed(void **out_retval,
                                         const char **out_exc_class,
                                         const char **out_exc_message);
void    oxphp_async_sched_release(int64_t fiber_id);
int     oxphp_async_sched_cancel(int64_t fiber_id);

/* Destroy this thread's task scheduler (frees fiber C stacks + task payload).
 * Called from the extension RSHUTDOWN; a no-op if no task ever spawned. */
void    oxphp_async_sched_shutdown(void);

#endif /* OXPHP_FIBER_H */
