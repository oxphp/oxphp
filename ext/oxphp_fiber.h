/* ext/oxphp_fiber.h — Fiber-based request multiplexing scheduler. */
#ifndef OXPHP_FIBER_H
#define OXPHP_FIBER_H

#include "php.h"
#include "main/php_output.h" /* php_output_handler: a suspended request parks its own */
#include "zend_fibers.h"

#include <poll.h>

/* Maximum concurrent fibers per worker thread. */
#define OXPHP_MAX_FIBERS 256

/* Maximum descriptors one fiber may wait on in a single suspension. Sized after
 * the ceiling PHP itself imposes on a multiplexed wait — its select-based path
 * refuses descriptors at or past FD_SETSIZE, which is 1024 on Linux — so that a
 * wait PHP would have accepted is never turned away here. A caller waiting on
 * that many does not put them on its stack; the array only has to outlive the
 * suspension, so a heap buffer freed after the wait is equally valid.
 *
 * Deliberately not stored inside the fiber: an inline array of this width would
 * cost 8 KiB per fiber, 2 MiB per worker thread, to spare a pointer whose
 * lifetime is already pinned by the suspension. */
#define OXPHP_MAX_WAIT_FDS 1024

struct _oxphp_fiber_scheduler;
struct oxphp_io_reg;

/* Which fiber, and which of its descriptors, a readiness registration belongs
 * to — so an event can be scattered back into that fiber's own descriptor
 * array, where the waiting code reads it. One per descriptor, living beside the
 * descriptors themselves. */
struct oxphp_io_owner {
    struct _oxphp_request_fiber *fiber;
    uint32_t idx;
};

/* ─── Suspend Reasons ──────────────────────────────────── */

typedef enum {
    OXPHP_SUSPEND_NONE = 0,
    OXPHP_SUSPEND_AWAIT,        /* waiting for oxphp_async_await result */
    OXPHP_SUSPEND_AWAIT_ALL,    /* waiting for oxphp_async_await_all */
    OXPHP_SUSPEND_AWAIT_ANY,    /* waiting for oxphp_async_await_any */
    OXPHP_SUSPEND_SLEEP,        /* waiting for oxphp_sleep timer */
    OXPHP_SUSPEND_IO_WAIT,      /* waiting for readiness of a file descriptor */
} oxphp_suspend_reason;

/* A suspend point returns this when its resume delivered a pending exception:
 * the caller must return to PHP at once and add no exception of its own. */
#define OXPHP_FIBER_UNWIND (-9)

/* ─── Per-Fiber PHP State ──────────────────────────────── */

#define OXPHP_SYMBOL_GLOBAL_COUNT 6

struct oxphp_symbol_global_name {
    const char *name;
    size_t len;
};

/* Defined in oxphp_fiber.c; indexes php_state.symbol_globals. */
extern const struct oxphp_symbol_global_name
    oxphp_symbol_global_names[OXPHP_SYMBOL_GLOBAL_COUNT];

/* Written on suspend only (oxphp_fiber_save_php_state) and read only while that
 * fiber is suspended — see oxphp_scheduler_start_fiber for why handing a fiber a
 * NEW request restores none of it. Holds no Zend VM state: zend_fiber_switch_context
 * carries that itself, per switching frame. */
typedef struct {
    /* Superglobals: saved PG(http_globals) values */
    zval http_globals[6]; /* TRACK_VARS_POST .. TRACK_VARS_FILES */

    /* What userland actually reads: the EG(symbol_table) entries for _POST,
     * _GET, _COOKIE, _SERVER, _FILES and _REQUEST, in the order of
     * oxphp_symbol_global_names. Not derivable from http_globals above — the
     * first `$_GET['x'] = …` separates the array by COW (the slot and the table
     * share it, so its refcount is at least two) and only the table gets the
     * written copy. Each entry is owned while set, IS_UNDEF when the fiber holds
     * none. */
    zval symbol_globals[6];

    /* SAPI state */
    zend_llist sapi_headers;
    /* The content type the engine recorded for this response, which output
     * handlers (mbstring, iconv) read to decide whether to convert what a
     * request writes. Thread-global like the rest of SG(sapi_headers), so a
     * suspended request would otherwise resume reading whichever request
     * touched it last. Owned while set. */
    char *sapi_mimetype;
    int http_response_code;
    bool headers_sent;

    /* What php://input reads: the buffered body stream the engine opened for
     * this request, the number of body bytes the SAPI has handed over so far,
     * and the flag saying there are no more. All three are thread-wide, and
     * every new request resets them — so a request that suspends would
     * otherwise resume reading the body of whichever request the worker served
     * in the window, and read it verbatim: with post_read set, php://input never
     * asks the SAPI for this request's bytes at all, it rewinds the stream it
     * finds standing here and reads that one.
     *
     * The stream travels by move like the rest of this struct, and the request
     * owns it: the resource list that closes it in every other SAPI belongs to
     * the WORKER here, not to the request, so a request that did not close its
     * own body would leave a whole copy of it standing for the life of the
     * worker. The close happens where the request ends
     * (oxphp_release_request_post_state), which is why the restore may drop
     * whatever stands in its place — that pointer belongs to a request that has
     * already been ended, and ending it is what set this one's field to NULL. */
    struct _php_stream *request_body;
    int64_t read_post_bytes;
    unsigned char post_read;

    /* The temp files rfc1867 wrote for this request's $_FILES, by which
     * is_uploaded_file() and move_uploaded_file() recognise a path as an upload
     * of the request asking. Thread-wide like the fields above, so a request
     * that suspends would otherwise come back holding whichever request's
     * uploads the worker took in the window — and have its own unlinked by that
     * request's end. Owned while parked; NULL when the request uploaded
     * nothing, which is the engine's own empty state. */
    HashTable *rfc1867_uploaded_files;

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

    /* The output buffers this request has open. The stack they sit on is
     * thread-wide, so a request that suspends with one open would otherwise
     * catch the echo of whichever request the worker serves meanwhile — one
     * client's content written into another's buffer, and ended into the wrong
     * response. Parked whole: the next request finds the empty stack a request
     * starts with, and this one gets its layers, their content and any handler a
     * script installed back on resume. Owned while parked. */
    zend_stack ob_handlers;
    php_output_handler *ob_active;
    php_output_handler *ob_running;

    /* What error_get_last() answers, which is thread-wide like the rest of this
     * struct. A request that suspends would otherwise come back reading the last
     * error of whichever request the worker served in the window — and reporting
     * it as its own, since reading this in a shutdown function to decide whether
     * the request died is exactly what frameworks and test harnesses do with it.
     * The two strings are owned while parked. */
    int last_error_type;
    int last_error_lineno;
    zend_string *last_error_message;
    zend_string *last_error_file;

    /* The shutdown functions this request has registered. The registry they sit
     * in is thread-wide, and the end of a request runs everything standing in it
     * and then frees the lot — so a request that suspends holding a registration
     * would have it run by whichever request the worker serves in the window,
     * into that request's response, and freed before its own end could run it.
     * Owned while parked; NULL when the request registered none, which is what
     * the engine's own empty state is. */
    HashTable *shutdown_functions;
} oxphp_fiber_php_state;

/* ─── Request Fiber ────────────────────────────────────── */

typedef struct _oxphp_request_fiber {
    /* The userland Fiber object this fiber runs as — one per HTTP request in
     * worker mode, one per oxphp_async() task. Owned: one reference from
     * creation until oxphp_scheduler_destroy. Running on a real Fiber object is
     * what makes \Fiber::getCurrent() inside a request or a task return an
     * object unique to it — and that in turn is what stops libraries keyed on
     * the current fiber (event loops, context storages, fiber-locals) from
     * filing every concurrent request or task under one key. */
    zend_fiber *zf;

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
            /* Borrowed on the same terms, from the same frame: one entry per
             * descriptor, holding the identity the readiness registration
             * carries back (which fiber, which of its descriptors). */
            struct oxphp_io_owner *owners;
            uint32_t nfds;
            bool expired;             /* deadline elapsed before any fd was ready */
            uint64_t deadline_ns;     /* CLOCK_MONOTONIC deadline, 0 = wait forever */
        } io;
    } suspend_data;

    /* The zend_fcall_info/cache for the handler closure (shared, not owned) */
    zend_fcall_info *fci;
    zend_fcall_info_cache *fcc;

    /* This fiber's C-stack bounds, estimated from the stack pointer at coroutine
     * entry (the fiber stack struct is opaque) and written exactly once, there.
     * NULL until then, which is how the switch wrappers tell a fresh fiber from
     * one that has run — see oxphp_fiber_install_stack_limits, which every path
     * into a fiber calls. */
    void *saved_stack_base;
    void *saved_stack_limit;

    /* Handler result tracking */
    bool handler_failed;
    bool completed;          /* set by coroutine before final switch — low-level API never sets DEAD */
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

    /* The scheduler that created this fiber — the HTTP one for a request fiber,
     * the task one for an async fiber. Distinct from `scheduler` above, which is
     * the context to switch back into. Needed because a fiber registers its own
     * descriptors from inside its own frame, where the only thing in reach is
     * the fiber; resolving it from thread-local state instead would depend on
     * which tick happens to be running. */
    struct _oxphp_fiber_scheduler *owner_sched;

    /* Linked list pointers for the scheduler's fiber list */
    struct _oxphp_request_fiber *next;
    struct _oxphp_request_fiber *prev;
} oxphp_request_fiber;

/* ─── Fiber Scheduler ──────────────────────────────────── */

typedef struct _oxphp_fiber_scheduler {
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

    /* Readiness backend for descriptor waits: one epoll instance per scheduler,
     * which is one per thread since both schedulers are thread-local, plus a
     * one-shot timer registered inside it so an idle wait can be bounded
     * without asking the kernel for a millisecond-granularity timeout. Created
     * on the first park and closed by oxphp_scheduler_destroy(); -1 until then,
     * so a worker whose requests never wait on a descriptor pays nothing. */
    int epfd;
    int timer_fd;

    /* Which registration each watched descriptor currently belongs to, so a
     * removal can be checked against the registration that made it — a
     * descriptor number can be closed and handed to a new connection while its
     * first waiter is still parked. Open-addressed on the descriptor, capacity
     * reg_mask + 1 (a power of two), NULL until the first park. Freed by
     * oxphp_scheduler_destroy(). Layout is private to oxphp_fiber.c. */
    struct oxphp_io_reg *reg_slots;
    uint32_t reg_mask;
    uint32_t reg_count;
} oxphp_fiber_scheduler;

/* ─── TLS: current fiber pointer ───────────────────────── */

/* Set when a fiber is executing, NULL in scheduler context.
 * Used by oxphp_async_await/oxphp_sleep to detect fiber mode. */
extern __thread oxphp_request_fiber *oxphp_current_fiber;

/* fiber_id of the currently executing request fiber, 0 outside fiber context.
 * Registered into the bridge at MINIT (oxphp_bridge_set_current_fiber_id_fn)
 * so Rust can tag async promise ownership at creation. */
uint64_t oxphp_fiber_current_id(void);

/* ─── Userland fiber object plumbing ───────────────────── */

/* Build the fabricated internal function that every request/task fiber runs as
 * its callable. Called once from the extension MINIT. */
void oxphp_fiber_minit(void);

/* Fill `fci`/`fcc` with a zero-argument call to that function. The fcc carries
 * the handler directly, so no name lookup happens and the function stays out of
 * the global function table — userland can neither see nor call it. */
void oxphp_fiber_loop_fci(zend_fcall_info *fci, zend_fcall_info_cache *fcc);

/* One-shot handoff: set immediately before zend_fiber_start(), consumed by the
 * loop handler on its first line. Safe because zend_fiber_start() enters the
 * fiber synchronously on the same thread with nothing running in between. */
extern __thread oxphp_request_fiber *oxphp_fiber_starting;

/* Names the fiber the scheduler is currently switching into, for the whole time
 * it is in there, and NULL otherwise. A fiber that wakes from a park to find the
 * token naming someone else was resumed by userland — a request's own \Fiber
 * object escapes through \Fiber::getCurrent() into any library that keys on the
 * running fiber, and one of them resumed it. That wake installs none of the
 * request's state and nothing in the scheduler is waiting on it, so the fiber
 * refuses it instead of running on. */
extern __thread oxphp_request_fiber *oxphp_fiber_resume_token;

/* ─── Context switch primitives ────────────────────────── */

/* Switch from the scheduler into `fiber`, delivering `value` (NULL = null).
 * Returns once the fiber suspends or parks at the bottom of its loop. Callers
 * are responsible for the C-stack-bound save/restore around it. */
void oxphp_fiber_enter(oxphp_request_fiber *fiber, zval *value);

/* Suspend the running fiber back to its scheduler. Called from the fiber's own
 * context only. Returns 0 when the fiber was resumed normally. */
int oxphp_fiber_park(oxphp_request_fiber *fiber);

/* ─── Scheduler API ────────────────────────────────────── */

void oxphp_scheduler_init(oxphp_fiber_scheduler *sched);
/* End the requests still parked on a worker that is retiring, each on its own
 * state and into its own response, before the teardown below — which cannot.
 * Request schedulers only: it finalizes what it unwinds, and finalizing sends a
 * response. */
void oxphp_scheduler_retire_fibers(oxphp_fiber_scheduler *sched);
void oxphp_scheduler_destroy(oxphp_fiber_scheduler *sched);

oxphp_request_fiber *oxphp_scheduler_create_fiber(
    oxphp_fiber_scheduler *sched,
    zend_fcall_info *fci,
    zend_fcall_info_cache *fcc
);

/* Run a NEW request on `fiber`, fresh or recycled. Restores nothing from
 * php_state — that snapshot describes a suspended request only. */
void oxphp_scheduler_start_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber);
/* Resume a fiber SUSPENDED mid-request, re-installing the state it saved. */
void oxphp_scheduler_resume_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber, zval *value);
void oxphp_scheduler_finalize_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber);

/* Start watching a fiber's descriptors, so the scheduler can resolve their
 * readiness while the fiber is suspended. Returns false when the set cannot be
 * watched at all, which the caller must read as "this fiber may not park" — the
 * wait then falls to the caller's own blocking path. Takes the set as arguments
 * rather than reading it off the fiber, so a refusal leaves no half-written
 * suspension to undo. */
bool oxphp_io_park(oxphp_request_fiber *fiber, struct pollfd *fds,
                   struct oxphp_io_owner *owners, uint32_t nfds);

/* Stop watching them. Reads the set off the fiber, so it must run before the
 * suspension data is cleared. */
void oxphp_io_unpark(oxphp_request_fiber *fiber);

/* ─── Which fiber a connection belongs to ─────────────────
 * Keeps one fiber's exchange on a connection out of another's while the first is
 * parked mid-exchange — see the block comment on the implementation for why a
 * shared connection needs this and what it costs. The key is whatever names the
 * connection at the level being guarded: a `php_stream *` for the socket ops, and
 * for the database entry points — which have to be guarded a level up, because
 * their client refuses a reentrant call before any I/O — the driver's own
 * connection handle where it can be reached, the client object otherwise. Used by
 * the runtime hooks only; inert when nothing ever claims anything, which is every
 * mode with the hooks off. */

/* The fiber currently holding `key`, or NULL when no one does. */
oxphp_request_fiber *oxphp_claim_owner(void *key);

/* Record `owner` as holding `key`. The caller must have established that it is
 * unclaimed or already its own — this does not arbitrate. Returns false only when
 * the table could not grow, which leaves the connection unprotected and is the
 * caller's to report. */
bool oxphp_claim_acquire(void *key, oxphp_request_fiber *owner);

/* Forget `key` entirely, whoever held it. Called when a stream is closed, so that
 * an address handed out again starts unclaimed. */
void oxphp_claim_forget(void *key);

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

/* Build a worker-mode request's input. Called in this order, and from inside the
 * request's own fiber (oxphp_fiber_loop_handler) rather than from the prep that
 * runs on the worker's stack, because the middle step runs PHP: the body parse
 * raises its limit diagnostics through the application's set_error_handler.
 *
 *   1. oxphp_reset_request_post_state()      — give back the SAPI post state of
 *      whoever ran last, so that nothing of theirs is still registered when the
 *      auto-global callbacks below ask the SAPI for a parsed body.
 *   2. oxphp_reset_request_context_globals() — $_GET, $_COOKIE and above all
 *      $_SERVER, complete with REQUEST_TIME. $_POST and $_FILES come out of this
 *      empty; step 3 fires them again once there is a body to build them from.
 *   3. oxphp_reset_request_body_globals()    — read the body, then $_POST,
 *      $_FILES and $_REQUEST. Everything it raises reads step 2's $_SERVER.
 *
 * The counterpart is the end of the request, which destroys all of it
 * (oxphp_scheduler_finalize_fiber): between two requests a worker holds no
 * request's superglobals at all, so there is nothing for the next one — or for
 * anything running in the gap — to read as its own. */
void oxphp_reset_request_post_state(void);
void oxphp_reset_request_context_globals(void);
void oxphp_reset_request_body_globals(void);

/* Release what the body parse left: the uploaded-file temp files and the
 * buffered body stream. The counterpart of the call above, at the end of the
 * request rather than the start, because that is the only point at which the
 * fields in SG() are known to describe the request being ended. */
void oxphp_release_request_post_state(void);

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
