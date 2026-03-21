/* ext/oxphp_fiber.h — Fiber-based request multiplexing scheduler. */
#ifndef OXPHP_FIBER_H
#define OXPHP_FIBER_H

#include "php.h"
#include "zend_fibers.h"

/* Maximum concurrent fibers per worker thread. */
#define OXPHP_MAX_FIBERS 256

/* ─── Suspend Reasons ──────────────────────────────────── */

typedef enum {
    OXPHP_SUSPEND_NONE = 0,
    OXPHP_SUSPEND_AWAIT,        /* waiting for oxphp_async_await result */
    OXPHP_SUSPEND_AWAIT_ALL,    /* waiting for oxphp_async_await_all */
    OXPHP_SUSPEND_AWAIT_ANY,    /* waiting for oxphp_async_await_any */
    OXPHP_SUSPEND_SLEEP,        /* waiting for oxphp_sleep timer */
} oxphp_suspend_reason;

/* ─── Per-Fiber Saved PHP State ────────────────────────── */

/* ─── Per-Fiber VM State ───────────────────────────────── */

/* Replicates zend_fiber_vm_state from zend_fibers.c (internal/static).
 * The low-level fiber API (zend_fiber_switch_context) does NOT save VM state —
 * only the high-level API (zend_fiber_start/resume) does. We must do it
 * ourselves for each context switch. */
typedef struct {
    zend_vm_stack vm_stack;
    zval *vm_stack_top;
    zval *vm_stack_end;
    zend_execute_data *current_execute_data;
    int error_reporting;
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

    /* Linked list pointers for the scheduler's fiber list */
    struct _oxphp_request_fiber *next;
    struct _oxphp_request_fiber *prev;
} oxphp_request_fiber;

/* ─── Fiber Scheduler ──────────────────────────────────── */

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
} oxphp_fiber_scheduler;

/* ─── TLS: current fiber pointer ───────────────────────── */

/* Set when a fiber is executing, NULL in scheduler context.
 * Used by oxphp_async_await/oxphp_sleep to detect fiber mode. */
extern __thread oxphp_request_fiber *oxphp_current_fiber;

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

/* Save/restore PHP state around context switches. */
void oxphp_fiber_save_php_state(oxphp_request_fiber *fiber);
void oxphp_fiber_restore_php_state(oxphp_request_fiber *fiber);

/* Targeted per-fiber request init (safe to call while other fibers are suspended).
 * Unlike oxphp_soft_reset(), this does NOT touch global OB or other thread-wide state.
 * It only initializes fresh superglobals and SAPI headers for the new request. */
void oxphp_fiber_init_request_state(void);

#endif /* OXPHP_FIBER_H */
