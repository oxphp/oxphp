/* ext/oxphp_fiber.c — Fiber scheduler implementation.
 *
 * Provides cooperative multitasking for OxPHP worker mode. Each HTTP request
 * runs in its own zend_fiber_context. When a request calls oxphp_async_await()
 * or oxphp_sleep(), the fiber suspends and the scheduler resumes another fiber
 * or accepts new work.
 *
 * Key design:
 * - Low-level zend_fiber_context API (not PHP Fiber class)
 * - Fiber pointer stored in context->kind (read via EG(current_fiber_context)->kind)
 * - Zend VM state (vm_stack, execute_data, bailout, error_reporting,
 *   jit_trace_num, active_fiber) is carried by zend_fiber_switch_context itself:
 *   it captures EG into a local on the frame that switches and restores it when
 *   that frame resumes, so each side of a switch keeps its own state and this
 *   file does not save or restore any of it
 * - PHP superglobals, SAPI headers, Rust TLS managed explicitly per fiber */

#include "oxphp_fiber.h"
#include "bridge/oxphp_bridge.h"

#include "SAPI.h"
#include "Zend/zend_exceptions.h"
#include "main/php_main.h"
#include "main/php_output.h"
#include "ext/standard/basic_functions.h"
#include <unistd.h> /* sysconf(_SC_PAGESIZE) for fiber stack limits */
#include <string.h> /* strdup/strndup/strstr for async-task exception capture */
#include <time.h>   /* clock_gettime/CLOCK_MONOTONIC for per-call await deadlines */
#include <poll.h>   /* struct pollfd: how a waiter states its interest and reads the outcome */
#include <sys/epoll.h>   /* the readiness backend for IO_WAIT-suspended fibers */
#include <sys/timerfd.h> /* periodic timer that bounds an idle wait inside epoll */
#include <errno.h>  /* EINTR from the readiness wait */
#include <stdatomic.h> /* one-shot flag for the readiness-wait failure log */

/* ─── TLS: current fiber pointer ───────────────────────── */

__thread oxphp_request_fiber *oxphp_current_fiber = NULL;

uint64_t oxphp_fiber_current_id(void) {
    return oxphp_current_fiber ? oxphp_current_fiber->fiber_id : 0;
}

/* ─── Userland fiber object plumbing ───────────────────── */

__thread oxphp_request_fiber *oxphp_fiber_starting = NULL;

/* The callable every request/task fiber runs. Deliberately never registered in
 * CG(function_table): zend_call_function skips name resolution when the cache
 * already carries a handler, so this stays reachable only from here — a script
 * can neither see it in get_defined_functions() nor call it by name. */
static zend_internal_function oxphp_fiber_loop_fn;

static ZEND_NAMED_FUNCTION(oxphp_fiber_loop_handler);

void oxphp_fiber_minit(void) {
    memset(&oxphp_fiber_loop_fn, 0, sizeof(oxphp_fiber_loop_fn));
    oxphp_fiber_loop_fn.type = ZEND_INTERNAL_FUNCTION;
    oxphp_fiber_loop_fn.fn_flags = ZEND_ACC_PUBLIC;
    oxphp_fiber_loop_fn.function_name =
        zend_string_init_interned("oxphp fiber loop", sizeof("oxphp fiber loop") - 1, 1);
    oxphp_fiber_loop_fn.handler = oxphp_fiber_loop_handler;
}

/* ZVAL_UNDEF on function_name is load-bearing: zend_fiber_execute and
 * zend_fiber_object_free both zval_ptr_dtor it, and that is a no-op on UNDEF. */
void oxphp_fiber_loop_fci(zend_fcall_info *fci, zend_fcall_info_cache *fcc) {
    memset(fci, 0, sizeof(*fci));
    memset(fcc, 0, sizeof(*fcc));
    fci->size = sizeof(*fci);
    ZVAL_UNDEF(&fci->function_name);
    fcc->function_handler = (zend_function *)&oxphp_fiber_loop_fn;
}

/* ─── Forward declarations ─────────────────────────────── */

static void oxphp_claim_release_fiber(const oxphp_request_fiber *fiber);
static void oxphp_claim_reset_if_empty(void);

/* ─── Stack Limit Helper ──────────────────────────────── */

/* zend_fiber_stack is an opaque (incomplete) type — we cannot access its
 * fields. Instead, we estimate the stack boundaries from the fiber's
 * configured stack size and the address of a local variable on the fiber's
 * C stack. Called from the coroutine entry point. */
static inline void oxphp_fiber_set_stack_limits_from_sp(void *stack_local, size_t stack_size) {
    size_t page_size = (size_t)sysconf(_SC_PAGESIZE);
    uintptr_t sp = (uintptr_t)stack_local;

    /* Round up to page boundary for base (top of stack) */
    EG(stack_base) = (void *)((sp + page_size - 1) & ~(page_size - 1));
    /* limit = base - usable_size + guard page */
    EG(stack_limit) = (void *)((uintptr_t)EG(stack_base) - stack_size + page_size);
}

/* Install a fiber's C-stack bounds before switching into it, from the copy its
 * coroutine measured at entry (NULL = a fresh fiber, which measures its own on
 * the way in). Called on every path into a fiber so they all read alike.
 *
 * The engine carries these two across a switch itself, but only under
 * ZEND_CHECK_STACK_LIMIT — a configure-time guard, on fields that
 * zend_globals.h declares unconditionally. Keeping the install makes the
 * invariant local instead of upstream's to withdraw. */
static inline void oxphp_fiber_install_stack_limits(const oxphp_request_fiber *fiber) {
    if (fiber->saved_stack_base) {
        EG(stack_base) = fiber->saved_stack_base;
        EG(stack_limit) = fiber->saved_stack_limit;
    }
}

/* ─── Context switch primitives ────────────────────────── */

/* The only two places the engine's context switch is called. Every path into a
 * fiber goes through the first and every park goes through the second, so the
 * pair is the single seam where the switching mechanism can be changed. */

void oxphp_fiber_enter(oxphp_request_fiber *fiber, zval *value) {
    if (fiber->zf == NULL) {
        /* Bare-context fiber (task mode). Switches straight to the context;
         * nothing tracks it as the engine's active fiber. */
        zend_fiber_transfer transfer = { .context = &fiber->context, .flags = 0 };
        if (value) {
            ZVAL_COPY_VALUE(&transfer.value, value);
        } else {
            ZVAL_NULL(&transfer.value);
        }

        zend_fiber_switch_context(&transfer);
        return;
    }

    zval retval;
    ZVAL_UNDEF(&retval);

    if (fiber->zf->context.status == ZEND_FIBER_STATUS_INIT) {
        /* First entry allocates the C stack and the VM stack and runs the loop
         * up to its first park. The handoff is read on the handler's first
         * line. */
        oxphp_fiber_starting = fiber;
        if (zend_fiber_start(fiber->zf, &retval) == FAILURE) {
            oxphp_fiber_starting = NULL;
            fiber->completed = true;
            fiber->handler_failed = true;
            return;
        }
    } else {
        zend_fiber_resume(fiber->zf, value, &retval);
    }

    zval_ptr_dtor(&retval);

    /* The loop catches everything a handler can throw, so an exception arriving
     * here is the fiber itself unwinding (destruction, bailout forwarding). The
     * scheduler has no PHP frame to unwind into, so report and clear rather than
     * leaving it pending for whatever runs next. */
    if (UNEXPECTED(EG(exception))) {
        zend_clear_exception();
    }
}

int oxphp_fiber_park(oxphp_request_fiber *fiber) {
    if (fiber->zf == NULL) {
        zend_fiber_transfer transfer = { .context = fiber->scheduler, .flags = 0 };
        ZVAL_NULL(&transfer.value);

        zend_fiber_switch_context(&transfer);

        return 0;
    }

    /* A destroyed fiber must not park again — zend_fiber_suspend asserts on it,
     * and the caller's contract is to unwind instead. */
    if (UNEXPECTED(fiber->zf->flags & ZEND_FIBER_FLAG_DESTROYED)) {
        return -1;
    }

    zval retval;
    ZVAL_UNDEF(&retval);
    zend_fiber_suspend(fiber->zf, NULL, &retval);
    zval_ptr_dtor(&retval);

    /* zend_fiber_suspend delivers a resume that carries an exception by leaving
     * it pending here (graceful exit from destruction, or a throw from a
     * userland resumer). Report it to the caller, which returns to PHP at once
     * and lets the VM unwind. */
    if (UNEXPECTED(EG(exception))) {
        return -1;
    }

    return 0;
}

/* ─── Scheduler Init / Destroy ─────────────────────────── */

void oxphp_scheduler_init(oxphp_fiber_scheduler *sched) {
    memset(sched, 0, sizeof(*sched));
    sched->next_fiber_id = 1;
    sched->epfd = -1;
    sched->timer_fd = -1;
}

/* Free per-fiber owned state at teardown: the task-mode payload (reconstructed
 * closure, captured return value, captured-exception strings) and any parked
 * unhandled-exception capture. The borrowed task_args belong to the Rust driver
 * and are NOT touched here. The task_* fields are a no-op for HTTP request
 * fibers (they stay zeroed/UNDEF); php_state.unhandled_exc is a no-op for
 * task-mode fibers (only HTTP request fibers ever park one) and is normally NULL
 * even for request fibers — non-NULL only when a request fiber is destroyed
 * while suspended in a shutdown function with a capture parked. Single source of
 * truth shared by oxphp_async_sched_release and oxphp_scheduler_destroy so all
 * paths free the same set of fields. */
static void oxphp_fiber_free_task_payload(oxphp_request_fiber *fiber) {
    if (!Z_ISUNDEF(fiber->task_closure)) {
        zval_ptr_dtor(&fiber->task_closure);
        ZVAL_UNDEF(&fiber->task_closure);
    }
    if (!Z_ISUNDEF(fiber->task_retval)) {
        zval_ptr_dtor(&fiber->task_retval);
        ZVAL_UNDEF(&fiber->task_retval);
    }
    if (fiber->task_exc_class) {
        free(fiber->task_exc_class);
        fiber->task_exc_class = NULL;
    }
    if (fiber->task_exc_message) {
        free(fiber->task_exc_message);
        fiber->task_exc_message = NULL;
    }
    if (fiber->php_state.unhandled_exc) {
        oxphp_bridge_free_unhandled(fiber->php_state.unhandled_exc);
        fiber->php_state.unhandled_exc = NULL;
    }
    /* Request-fiber state, like unhandled_exc above: only a fiber destroyed
     * while parked still holds these, since resuming hands them back to the
     * symbol table. Released here so worker teardown does not leak the arrays. */
    for (size_t i = 0; i < OXPHP_SYMBOL_GLOBAL_COUNT; i++) {
        if (!Z_ISUNDEF(fiber->php_state.symbol_globals[i])) {
            zval_ptr_dtor(&fiber->php_state.symbol_globals[i]);
            ZVAL_UNDEF(&fiber->php_state.symbol_globals[i]);
        }
    }
}

/* Release a fiber's execution resources — its C stack, and for a request fiber
 * the VM stack and the Fiber object it runs as. Called at scheduler teardown,
 * where a request fiber can still be suspended in the middle of its request.
 *
 * The Fiber object's destructor is what resumes such a fiber with a graceful
 * exit, so the request unwinds through the loop's zend_try instead of being
 * abandoned with its C and VM stacks live. It is called explicitly rather than
 * left to the reference drop below, because the drop is not guaranteed to reach
 * it: a request that handed \Fiber::getCurrent() to something outliving the
 * request keeps the object alive past that drop, and an unwind deferred to then
 * would run on a loop frame whose fiber pointer the caller has already freed.
 *
 * Async task fibers still run as bare contexts and have no object to destroy. */
static void oxphp_fiber_release(oxphp_request_fiber *fiber) {
    if (!fiber->zf) {
        zend_fiber_destroy_context(&fiber->context);
        return;
    }

    zend_object *obj = &fiber->zf->std;
    if (!(OBJ_FLAGS(obj) & IS_OBJ_DESTRUCTOR_CALLED)) {
        GC_ADD_FLAGS(obj, IS_OBJ_DESTRUCTOR_CALLED);
        obj->handlers->dtor_obj(obj);
    }
    OBJ_RELEASE(obj);
    fiber->zf = NULL;
}

void oxphp_scheduler_destroy(oxphp_fiber_scheduler *sched) {
    /* Free any remaining active fibers */
    oxphp_request_fiber *fiber = sched->fibers_head;
    while (fiber) {
        oxphp_request_fiber *next = fiber->next;
        /* Before the bridge context and the parked state below: the unwind runs
         * PHP, which still reads both. */
        oxphp_fiber_release(fiber);
        /* And after it, for the same reason: the unwind is PHP, so the request
         * is still using — and can still take — a socket stream on its way out.
         * A fiber suspended mid-request may hold claims whose entries would name
         * freed memory once it is gone; fibers on the free list below released
         * theirs by completing. */
        oxphp_claim_release_fiber(fiber);
        oxphp_bridge_fiber_drop_ctx(fiber->fiber_id);
        oxphp_fiber_free_task_payload(fiber);
        efree(fiber);
        fiber = next;
    }
    /* Free the free list. Recycled fibers keep a live (suspended) looping
     * coroutine, so their mmap'd C stack must be released too — a bare efree
     * would leak the stack. */
    fiber = sched->free_list;
    while (fiber) {
        oxphp_request_fiber *next = fiber->next;
        oxphp_fiber_release(fiber);
        oxphp_fiber_free_task_payload(fiber);
        efree(fiber);
        fiber = next;
    }
    sched->fibers_head = NULL;
    sched->fibers_tail = NULL;
    sched->free_list = NULL;
    sched->fiber_count = 0;

    free(sched->reg_slots);
    sched->reg_slots = NULL;
    sched->reg_mask = 0;
    sched->reg_count = 0;

    /* Stream claims are thread-local, not per scheduler, so the table can only
     * go once nothing holds an entry any more — on a thread running both a
     * request scheduler and a task scheduler, the first of them to be destroyed
     * must not take the other's claims with it. With one scheduler per thread,
     * which is the usual case, the loops above emptied it. */
    oxphp_claim_reset_if_empty();

    /* After the fibers, not before: closing the instance drops every
     * registration they still held, so nothing can report readiness for a fiber
     * that has already been freed. */
    if (sched->timer_fd >= 0) {
        close(sched->timer_fd);
        sched->timer_fd = -1;
    }
    if (sched->epfd >= 0) {
        close(sched->epfd);
        sched->epfd = -1;
    }
}

/* ─── Coroutine Entry Point ────────────────────────────── */

/* Looping coroutine: the fiber's C stack is allocated ONCE and reused for all
 * requests assigned to this fiber. After each request completes, the coroutine
 * suspends back to the scheduler (marking completed=true). The scheduler can
 * then resume it for the next request without mmap/munmap overhead.
 *
 * If the handler suspends mid-request (oxphp_sleep/oxphp_async_await), the
 * scheduler creates additional fibers for concurrent requests. */

/* Capture the in-flight exception into bridge TLS for the worker send path, so
 * the root SERVER span can carry it without any PHP-side integration.
 * EG(exception) must be live (called before OBJ_RELEASE). Reads file/line from
 * the Throwable's own properties (throw origin); oxphp_exception_capture handles
 * class (borrowed, length-delimited), message + getTraceAsString (malloc'd). */
static void oxphp_capture_unhandled(zend_object *ex) {
    const char *cls = NULL;
    char *msg = NULL, *trace = NULL;
    size_t cls_len = 0, msg_len = 0, trace_len = 0;
    oxphp_exception_capture(ex, &cls, &cls_len, &msg, &msg_len, &trace, &trace_len);

    zval rv_f, rv_l;
    zval *fz = zend_read_property(ex->ce, ex, "file", sizeof("file") - 1, 1, &rv_f);
    zval *lz = zend_read_property(ex->ce, ex, "line", sizeof("line") - 1, 1, &rv_l);
    const char *file = (fz && Z_TYPE_P(fz) == IS_STRING) ? Z_STRVAL_P(fz) : NULL;
    size_t file_len = (fz && Z_TYPE_P(fz) == IS_STRING) ? Z_STRLEN_P(fz) : 0;
    uint32_t line = (lz && Z_TYPE_P(lz) == IS_LONG) ? (uint32_t)Z_LVAL_P(lz) : 0;

    oxphp_bridge_set_unhandled_exc(cls, cls_len, msg, msg_len, trace, trace_len,
                                   file, file_len, line);
    free(msg);
    free(trace);
}

/* The loop every request fiber runs, as the body of the fabricated callable the
 * engine starts it with. Its C stack is allocated once by zend_fiber_start and
 * reused for every request assigned to this fiber: after each one the loop parks
 * back to the scheduler, which resumes it for the next without mmap/munmap.
 *
 * The frame this runs in never returns while the fiber is alive, which is why
 * the fiber pointer can live in it — no back-pointer on the context is needed.
 *
 * If the handler suspends mid-request (oxphp_sleep/oxphp_async_await), the
 * scheduler creates additional fibers for concurrent requests. */
static ZEND_NAMED_FUNCTION(oxphp_fiber_loop_handler) {
    (void)execute_data;
    (void)return_value;

    /* Consume the one-shot handoff: zend_fiber_start entered this fiber
     * synchronously, so the pointer set immediately before it is still ours. */
    oxphp_request_fiber *fiber = oxphp_fiber_starting;
    oxphp_fiber_starting = NULL;
    ZEND_ASSERT(fiber != NULL);

    fiber->scheduler = fiber->zf->caller;

    /* Set stack overflow detection limits ONCE (C stack is reused) */
    int stack_anchor;
    oxphp_fiber_set_stack_limits_from_sp(&stack_anchor, EG(fiber_stack_size));
    fiber->saved_stack_base = EG(stack_base);
    fiber->saved_stack_limit = EG(stack_limit);

    /* ── Request processing loop ────────────────────────── */
    for (;;) {
        oxphp_current_fiber = fiber;

        /* No VM stack management here. zend_fiber_execute built this fiber's VM
         * stack before calling us and pointed zf->stack_bottom into it, and
         * every zend_fiber_resume writes through that pointer — so destroying
         * the stack per request would hand the engine freed memory on the next
         * resume. One stack now lives as long as the fiber does. */

        /* Start each request uncoupled from any trace recording left in flight
         * by the previous one on this fiber. zend_fiber_execute does this once
         * on entry; the loop needs it per iteration. */
        EG(jit_trace_num) = 0;

        /* Call the PHP handler with zend_try protection */
        zval retval;
        ZVAL_UNDEF(&retval);

        zend_try {
            fiber->fci->retval = &retval;
            fiber->fci->param_count = 0;
            fiber->fci->params = NULL;
            if (zend_call_function(fiber->fci, fiber->fcc) == SUCCESS) {
                zval_ptr_dtor(&retval);
            }
            if (EG(exception)) {
                if (!zend_is_unwind_exit(EG(exception)) && !zend_is_graceful_exit(EG(exception))) {
                    fiber->handler_failed = true;
                    /* Normal (non-bailout) unwind of an uncaught handler
                     * exception — the worker's only path to observe it, since
                     * the fiber swallows it before zend_exception_error runs.
                     * Capture the live object for the root span before release.
                     * (The zend_catch arm below is the zend_bailout/fatal path:
                     * fatals are already recorded by oxphp_error_cb, and calling
                     * getTraceAsString there could re-bailout — so no capture
                     * there.) */
                    oxphp_capture_unhandled(EG(exception));
                }
                OBJ_RELEASE(EG(exception));
                EG(exception) = NULL;
            }
        } zend_catch {
            fiber->handler_failed = true;
            if (EG(exception)) {
                OBJ_RELEASE(EG(exception));
                EG(exception) = NULL;
            }
            CG(unclean_shutdown) = 0;
        } zend_end_try();

        php_call_shutdown_functions();
        php_free_shutdown_functions();

        /* The request is over, so any socket stream this fiber claimed is free
         * for the next fiber that wants it. Here rather than in finalize because
         * this point is inside the zend_try above's reach: it is passed on every
         * outcome, an uncaught exception and a bailout included, and it is the
         * one place both request dispatch paths share. */
        oxphp_claim_release_fiber(fiber);

        oxphp_current_fiber = NULL;

        /* Mark completed and suspend back to scheduler.
         * Scheduler will resume us for the next request (looping coroutine). */
        fiber->completed = true;

        if (oxphp_fiber_park(fiber) != 0) {
            /* Resumed with an exception pending — the fiber is being torn down,
             * not handed another request. Return and let the engine unwind. */
            return;
        }

        /* ── HANDED THE NEXT REQUEST ─────────────────────────
         * A start, not a resume: nothing was restored from php_state, and this
         * request's own prep has already run (oxphp_scheduler_start_fiber). */
        fiber->completed = false;
        fiber->handler_failed = false;
    }
}

/* End a fiber's suspension. Every path that resumes a fiber goes through here
 * rather than assigning suspend_reason directly, because a descriptor wait has
 * bookkeeping to undo and there are four unrelated places that end one: the two
 * readiness passes, the drain sweep and the task cancellation pass. Keeping the
 * assignment in one place is what stops the fifth from being written without
 * it.
 *
 * Clearing the borrowed descriptor pointer matters: once the fiber resumes, the
 * frame that owned the array may return, and a stale pointer in a struct that
 * outlives it is the kind of thing a later reader will dereference. */
static inline void oxphp_fiber_clear_suspend(oxphp_request_fiber *fiber) {
    if (fiber->suspend_reason == OXPHP_SUSPEND_IO_WAIT) {
        /* Before the pointers are dropped: unparking reads the set to know what
         * to stop watching. */
        oxphp_io_unpark(fiber);
        fiber->suspend_data.io.fds = NULL;
        fiber->suspend_data.io.owners = NULL;
        fiber->suspend_data.io.nfds = 0;
    }
    fiber->suspend_reason = OXPHP_SUSPEND_NONE;
}

/* ─── Fiber Creation ───────────────────────────────────── */

oxphp_request_fiber *oxphp_scheduler_create_fiber(
    oxphp_fiber_scheduler *sched,
    zend_fcall_info *fci,
    zend_fcall_info_cache *fcc
) {
    oxphp_request_fiber *fiber;
    bool reused = false;

    if (sched->free_list) {
        /* Reuse from free list — C stack already allocated (looping coroutine).
         * Don't memset the whole struct — preserve context and stack limits. */
        fiber = sched->free_list;
        sched->free_list = fiber->next;
        reused = true;
    } else {
        /* Allocate new fiber + C stack (mmap — only happens once per fiber) */
        fiber = ecalloc(1, sizeof(oxphp_request_fiber));
    }

    fiber->fiber_id = sched->next_fiber_id++;
    fiber->owner_sched = sched;
    fiber->fci = fci;
    fiber->fcc = fcc;
    oxphp_fiber_clear_suspend(fiber);
    fiber->handler_failed = false;
    fiber->completed = false;
    fiber->consecutive_errors = 0;
    fiber->drain_kill = false;
    /* Capture this request's cancel cell here, in the one place both request
     * paths share: the Rust prep (setup_request_tls) installed it into the
     * bridge ctx immediately before this call — via worker_wait on the fast
     * path, via prepare_request on the event-loop path. Doing it at creation
     * also clears a free-list fiber's stale pointer from its previous request
     * (dangling once that request's CancellationState is dropped). Re-installed
     * into the bridge ctx on every resume — see oxphp_fiber_restore_php_state. */
    fiber->request_cancel_ptr = oxphp_bridge_get_cancel_ptr();

    if (!reused) {
        /* Create the Fiber object this request runs as. The C stack and the VM
         * stack are allocated by zend_fiber_start on first entry, from
         * oxphp_fiber_enter — not here. */
        zend_object *obj = zend_ce_fiber->create_object(zend_ce_fiber);
        if (!obj) {
            efree(fiber);
            return NULL;
        }
        fiber->zf = (zend_fiber *)obj;
        oxphp_fiber_loop_fci(&fiber->zf->fci, &fiber->zf->fci_cache);
    }
    /* Reused fibers: the loop is parked inside zend_fiber_suspend, waiting to be
     * resumed, and keeps both stacks. No re-init needed. */

    /* Add to scheduler's active list */
    fiber->prev = sched->fibers_tail;
    fiber->next = NULL;
    if (sched->fibers_tail) {
        sched->fibers_tail->next = fiber;
    } else {
        sched->fibers_head = fiber;
    }
    sched->fibers_tail = fiber;
    sched->fiber_count++;

    return fiber;
}

/* ─── State Save / Restore ─────────────────────────────── */

/* The superglobals a suspended request owns, in php_state.symbol_globals order.
 * _ENV is absent on purpose — it is process state in worker mode, not request
 * state, so a resuming fiber must leave the table's entry alone. */
const struct oxphp_symbol_global_name
    oxphp_symbol_global_names[OXPHP_SYMBOL_GLOBAL_COUNT] = {
        { ZEND_STRL("_POST") },   { ZEND_STRL("_GET") },
        { ZEND_STRL("_COOKIE") }, { ZEND_STRL("_SERVER") },
        { ZEND_STRL("_FILES") },  { ZEND_STRL("_REQUEST") },
};

void oxphp_fiber_save_php_state(oxphp_request_fiber *fiber) {
    /* ORDERING IS CRITICAL:
     * 1. Flush OB first — pushes any buffered output to ub_write → RESPONSE.output
     *    (must happen BEFORE saving Rust TLS so the output lands in the right buffer)
     * 2. Save Rust TLS (snapshots RESPONSE.output, EARLY_TX, REQUEST_DATA)
     * 3. Save PHP superglobals and SAPI headers */

    /* Step 1: Flush PHP OB to Rust RESPONSE.output */
    if (php_output_get_level() > 0) {
        php_output_flush_all();
    }

    /* Step 2: Save Rust TLS (RESPONSE, EARLY_TX, REQUEST_DATA, deadline) */
    oxphp_bridge_fiber_save_ctx(fiber->fiber_id);

    /* Step 3: Save superglobals */
    for (int i = 0; i < 6; i++) {
        ZVAL_COPY_VALUE(&fiber->php_state.http_globals[i], &PG(http_globals)[i]);
        ZVAL_UNDEF(&PG(http_globals)[i]); /* prevent double-free */
    }

    /* Step 3b: save what userland reads. The slots above are the engine's own
     * copies; a script reads EG(symbol_table), and the two part company the
     * moment the script writes — `$_GET['x'] = 1` finds the array shared with
     * the slot and separates it by COW, leaving the written copy in the table
     * only. The next request's reset then overwrites that entry, so without a
     * reference of our own this request's array (writes included) would drop to
     * refcount zero while it is still parked. Whatever wrapper the entry
     * carries is preserved as-is: `global $_GET` inside a function turns it into
     * an IS_REFERENCE, and handing the same reference back on resume keeps the
     * aliasing the function set up. */
    for (size_t i = 0; i < OXPHP_SYMBOL_GLOBAL_COUNT; i++) {
        zval_ptr_dtor(&fiber->php_state.symbol_globals[i]);
        ZVAL_UNDEF(&fiber->php_state.symbol_globals[i]);

        zval *entry = zend_hash_str_find(&EG(symbol_table),
                                         oxphp_symbol_global_names[i].name,
                                         oxphp_symbol_global_names[i].len);
        if (entry != NULL && !Z_ISUNDEF_P(entry)) {
            ZVAL_COPY(&fiber->php_state.symbol_globals[i], entry);
        }
    }

    /* Step 4: Save SAPI header state (move, not copy) */
    fiber->php_state.sapi_headers = SG(sapi_headers).headers;
    zend_llist_init(&SG(sapi_headers).headers,
                    sizeof(sapi_header_struct),
                    (void(*)(void*))sapi_free_header, 0);
    fiber->php_state.http_response_code = SG(sapi_headers).http_response_code;
    fiber->php_state.headers_sent = SG(headers_sent);
    fiber->php_state.connection_status = PG(connection_status);

    /* The thread-local bridge ctx is still THIS fiber's request here (the
     * fiber just suspended; nothing else ran yet) — capture its per-request
     * flags before the next multiplexed request's prep wipes them. */
    fiber->php_state.bridge_stream_mode = oxphp_bridge_is_streaming();
    fiber->php_state.bridge_headers_sent = oxphp_bridge_get_headers_sent();
    fiber->php_state.bridge_finished = oxphp_bridge_is_finished();

    /* Park any unhandled-exception capture with this fiber. A capture only
     * exists here when the handler already threw and a shutdown function is
     * suspending; taking it off the thread-active slot keeps the next request's
     * oxphp_bridge_reset_request_ctx from wiping it. NULL (the common case)
     * leaves php_state.unhandled_exc NULL. */
    fiber->php_state.unhandled_exc = oxphp_bridge_take_unhandled();
}

void oxphp_fiber_restore_php_state(oxphp_request_fiber *fiber) {
    /* Restore Rust TLS first (so ub_write goes to the right buffer) */
    oxphp_bridge_fiber_restore_ctx(fiber->fiber_id);

    /* Restore superglobals */
    for (int i = 0; i < 6; i++) {
        zval_ptr_dtor_nogc(&PG(http_globals)[i]); /* free current */
        ZVAL_COPY_VALUE(&PG(http_globals)[i], &fiber->php_state.http_globals[i]);
        ZVAL_UNDEF(&fiber->php_state.http_globals[i]);
    }

    /* Userland never reads PG(http_globals); it reads EG(symbol_table), whose
     * entries every new request's reset rebinds to that request's arrays. The
     * slot restore above therefore fixes nothing a script can see: without this
     * loop a request parked in a hooked sleep resumes reading the $_GET,
     * $_COOKIE and $_SERVER of whichever request the worker served meanwhile —
     * one client's parameters, cookies and headers read by another.
     *
     * Hand back the entries the save took rather than rebinding from the slots.
     * The slots are the wrong source: they hold the engine's pre-write copy of
     * each array, so restoring from them would also roll back every write the
     * request itself made before suspending, including when nothing else ran.
     * zend_hash_update transfers our reference to the table, which releases
     * whatever it was holding.
     *
     * _ENV is deliberately not in this set: it describes the process rather than
     * the request, so the table must keep what it already has. */
    for (size_t i = 0; i < OXPHP_SYMBOL_GLOBAL_COUNT; i++) {
        if (Z_ISUNDEF(fiber->php_state.symbol_globals[i])) {
            /* Absent at suspend must stay absent: an entry this request never
             * had can only have come from the request served in the window, and
             * $_REQUEST reaches that state whenever the window is what first
             * materializes it on this worker. */
            zend_hash_str_del(&EG(symbol_table),
                              oxphp_symbol_global_names[i].name,
                              oxphp_symbol_global_names[i].len);
            continue;
        }
        zend_hash_str_update(&EG(symbol_table),
                             oxphp_symbol_global_names[i].name,
                             oxphp_symbol_global_names[i].len,
                             &fiber->php_state.symbol_globals[i]);
        ZVAL_UNDEF(&fiber->php_state.symbol_globals[i]);
    }

    /* Restore SAPI headers */
    zend_llist_clean(&SG(sapi_headers).headers);
    SG(sapi_headers).headers = fiber->php_state.sapi_headers;
    zend_llist_init(&fiber->php_state.sapi_headers, /* reinit saved slot */
                    sizeof(sapi_header_struct),
                    (void(*)(void*))sapi_free_header, 0);
    SG(sapi_headers).http_response_code = fiber->php_state.http_response_code;
    SG(headers_sent) = fiber->php_state.headers_sent;
    PG(connection_status) = fiber->php_state.connection_status;

    /* The bridge's per-request ctx flags are thread-global, reset by every new
     * multiplexed request's prep (oxphp_bridge_reset_request_ctx) and not part
     * of the Rust-side fiber ctx save. Re-install this fiber's values so the
     * streaming flush path, oxphp_finish_request(), ub_write and the next
     * suspend's capture all see THIS request's state, not whichever request
     * touched the thread ctx last. */
    oxphp_bridge_set_stream_mode(fiber->php_state.bridge_stream_mode);
    oxphp_bridge_set_headers_sent(fiber->php_state.bridge_headers_sent);
    oxphp_bridge_set_finished(fiber->php_state.bridge_finished);

    /* Re-install this fiber's parked exception capture into the thread-active
     * slot (consumes the container). NULL is a no-op. */
    oxphp_bridge_restore_unhandled(fiber->php_state.unhandled_exc);
    fiber->php_state.unhandled_exc = NULL;

    /* Re-install this fiber's request cancel cell so the interrupt handler and
     * suspend points read THIS fiber's reason, not whichever request set the
     * per-thread pointer last. Without this, cancellation under fiber
     * multiplexing targets the wrong request. */
    oxphp_bridge_set_cancel_ptr(fiber->request_cancel_ptr);
}

/* ─── Shared per-request superglobal rebuild ───────────── */

void oxphp_reset_request_autoglobals(void) {
    /* zval_ptr_dtor_nogc skips the cycle collector — intentional: superglobals
     * are simple string arrays that never contain cyclic refs, and _nogc avoids
     * the cycle buffer insertion overhead on every request. */
    for (int i = 0; i < 6; i++) {
        zval_ptr_dtor_nogc(&PG(http_globals)[i]);
        ZVAL_UNDEF(&PG(http_globals)[i]);
    }

    /* Fires the non-JIT callbacks (_GET, _POST, _COOKIE, _FILES): they rebuild
     * PG(http_globals) and zend_hash_update the new arrays into
     * EG(symbol_table). The JIT ones (_SERVER, _ENV, _REQUEST) it only re-arms. */
    zend_activate_auto_globals();

    /* _ENV is the one auto-global that must not come back: its callback destroys
     * the array and repopulates it from environ, discarding what a .env loader
     * wrote straight into $_ENV at a worker boot that never runs again. Not
     * forcing it below is not enough to keep it — activate has just re-armed it,
     * and any zend_is_auto_global() for _ENV from anywhere then fires the
     * callback: compiling a file that mentions $_ENV, OPcache pinging one it
     * loads from cache, filter_input() on INPUT_ENV. Disarm it instead, and only
     * once the array exists, because until then the arm is what creates it. */
    if (zend_hash_exists(&EG(symbol_table), ZSTR_KNOWN(ZEND_STR_AUTOGLOBAL_ENV))) {
        zend_auto_global *env_global =
            zend_hash_find_ptr(CG(auto_globals), ZSTR_KNOWN(ZEND_STR_AUTOGLOBAL_ENV));
        if (env_global != NULL) {
            env_global->armed = false;
        }
    }

    /* Firing the JIT auto-globals that describe the request is this function's
     * job: OPcache would otherwise do it when it loads a script referencing
     * them, but only while the bit is still clear in a mask it resets in its own
     * request startup — which worker mode runs once per worker, so that re-fire
     * happens exactly once in a worker's life.
     *
     * ORDER IS LOAD-BEARING: _REQUEST must follow zend_activate_auto_globals(),
     * because php_auto_globals_create_request() merges
     * Z_ARRVAL(PG(http_globals)[GET/POST/COOKIE]) with no type check, and
     * activate is what makes those slots arrays again. Fired against the
     * IS_UNDEF slots the loop above leaves behind, it faults. */
    zend_is_auto_global(ZSTR_KNOWN(ZEND_STR_AUTOGLOBAL_SERVER));

    /* _REQUEST is forced only once something has materialized it. Until then no
     * loaded script mentions it, so there is nothing to read and nothing to go
     * stale; the first script that does mention it materializes it correctly at
     * compile time and leaves the symbol-table entry behind. The latch is what
     * makes the gate safe: `unset($_REQUEST)` at global scope removes that entry,
     * and a bare existence check would then skip for the rest of the worker's
     * life, because the OPcache mask that would otherwise re-fire the callback
     * never clears again in worker mode. */
    static __thread bool request_materialized = false;
    if (!request_materialized
        && zend_hash_exists(&EG(symbol_table),
                            ZSTR_KNOWN(ZEND_STR_AUTOGLOBAL_REQUEST))) {
        request_materialized = true;
    }
    if (request_materialized) {
        zend_is_auto_global(ZSTR_KNOWN(ZEND_STR_AUTOGLOBAL_REQUEST));
    }
}

/* ─── Targeted per-fiber request init ──────────────────── */

void oxphp_fiber_init_request_state(void) {
    /* Unlike oxphp_soft_reset(), this does NOT touch global OB or other
     * thread-wide state. It only initializes fresh superglobals and SAPI
     * headers for the new request. Safe to call while other fibers are
     * suspended with their state saved. */

    /* Clear SAPI headers for this new request */
    zend_llist_clean(&SG(sapi_headers).headers);
    SG(sapi_headers).http_response_code = 200;
    SG(sapi_headers).send_default_content_type = 1;
    SG(headers_sent) = 0;

    /* Reset SAPI post state */
    SG(read_post_bytes) = 0;
    SG(post_read) = 0;
    SG(request_info).request_body = NULL;

    /* Re-read cookies from the new request data */
    if (sapi_module.read_cookies) {
        SG(request_info).cookie_data = sapi_module.read_cookies();
    }

    /* Reset error state */
    PG(connection_status) = PHP_CONNECTION_NORMAL;

    /* Re-init superglobals from new request data */
    oxphp_reset_request_autoglobals();

    /* Inject REQUEST_TIME and REQUEST_TIME_FLOAT */
    double req_time = oxphp_bridge_get_request_time();
    if (Z_TYPE(PG(http_globals)[TRACK_VARS_SERVER]) == IS_ARRAY) {
        zval zt, ztf;
        ZVAL_LONG(&zt, (zend_long)req_time);
        ZVAL_DOUBLE(&ztf, req_time);
        zend_hash_str_update(Z_ARRVAL(PG(http_globals)[TRACK_VARS_SERVER]),
                             "REQUEST_TIME", sizeof("REQUEST_TIME") - 1, &zt);
        zend_hash_str_update(Z_ARRVAL(PG(http_globals)[TRACK_VARS_SERVER]),
                             "REQUEST_TIME_FLOAT", sizeof("REQUEST_TIME_FLOAT") - 1, &ztf);
    }
}

/* ─── Start / Resume / Finalize ────────────────────────── */

/* Hand a NEW request to `fiber` — either a fresh fiber whose coroutine starts at
 * its entry point, or a recycled one whose looping coroutine is parked at the
 * bottom of its loop waiting for the next request.
 *
 * Restores NOTHING from fiber->php_state, and holding that is why this is a
 * separate operation from resume: the snapshot describes a request that
 * SUSPENDED, while a fiber reaches the free list by COMPLETING one, so a
 * recycled fiber's copy belongs to nothing that is still running. Everything the
 * new request needs is already installed by the caller's per-request prep
 * (oxphp_soft_reset on the fast path, oxphp_bridge_prepare_request +
 * oxphp_fiber_init_request_state on the event-loop path); the coroutine builds a
 * fresh VM stack on every iteration, and zend_fiber_switch_context carries the
 * Zend side for both kinds of fiber.
 *
 * Resuming a genuinely suspended fiber is oxphp_scheduler_resume_fiber(). */
void oxphp_scheduler_start_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber) {
    sched->current = fiber;

    void *saved_stack_base = EG(stack_base);
    void *saved_stack_limit = EG(stack_limit);
    oxphp_fiber_install_stack_limits(fiber);

    oxphp_fiber_enter(fiber, NULL);

    EG(stack_base) = saved_stack_base;
    EG(stack_limit) = saved_stack_limit;
    sched->current = NULL;

    if (fiber->completed) {
        /* Handler completed without suspending */
        return;
    }

    /* Fiber suspended — snapshot its PHP state */
    oxphp_fiber_save_php_state(fiber);
}

void oxphp_scheduler_resume_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber, zval *value) {
    sched->current = fiber;

    /* Restore fiber's PHP state (superglobals, SAPI headers, Rust TLS) */
    oxphp_fiber_restore_php_state(fiber);

    oxphp_current_fiber = fiber;

    void *saved_stack_base = EG(stack_base);
    void *saved_stack_limit = EG(stack_limit);
    oxphp_fiber_install_stack_limits(fiber);

    oxphp_fiber_enter(fiber, value);

    EG(stack_base) = saved_stack_base;
    EG(stack_limit) = saved_stack_limit;
    oxphp_current_fiber = NULL;
    sched->current = NULL;

    if (!fiber->completed) {
        /* Suspended again — re-snapshot its PHP state */
        oxphp_fiber_save_php_state(fiber);
    }
}

void oxphp_scheduler_finalize_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber) {
    /* Precondition: this fiber's coroutine is parked at the bottom of its
     * request loop. Finalize both sends the response and recycles the fiber, and
     * oxphp_scheduler_start_fiber hands the next request straight to that park
     * point installing nothing, so finalizing one that is suspended mid-request
     * would drop a new request onto a fiber whose own is still live. Every
     * caller reaches here under `if (fiber->completed)`, so this only pins a
     * guarantee the callers already give — and only in a debug build, since
     * ZEND_ASSERT is ZEND_ASSUME once ZEND_DEBUG is off. The FFI-driven recycle
     * in oxphp_async_sched_release has no such caller discipline and carries a
     * real runtime guard instead. */
    ZEND_ASSERT(fiber->completed);

    /* Track per-fiber handler failure in scheduler-level counter. A drain
     * kill is an administrative unwind, not a handler defect: it must neither
     * trip the consecutive-error breaker (3+ drain kills would error-exit the
     * worker mid-drain, destroying still-live ordinary requests) nor reset it
     * (it says nothing about handler health). Set by the sweep below for
     * suspended fibers and by the interrupt handler for running ones. */
    if (fiber->drain_kill) {
        /* neutral */
    } else if (fiber->handler_failed) {
        sched->consecutive_errors++;
    } else {
        sched->consecutive_errors = 0;
    }
    /* Increment moved to request-start path (oxphp_scheduler_create_fiber
     * for the fast path; oxphp_scheduler_tick's new-request branch for
     * the event-loop path) so PHP-side requestCount() observes the
     * current request's index, not the previous count. */

    /* Cancel + drain async promises owned by THIS fiber before the response
     * is sent. Must be per-fiber: the thread-local promise maps are shared by
     * every fiber multiplexed on this worker thread, and a thread-wide drain
     * here would steal sibling fibers' live promises (their awaits would then
     * time out despite the tasks succeeding). */
    oxphp_bridge_cleanup_promises_for_fiber(fiber->fiber_id);

    /* Send the HTTP response via Rust (same as worker_send_callback) */
    oxphp_bridge_worker_send_response();

    /* Drop the fiber's Rust TLS slot (RESPONSE, EARLY_TX, REQUEST_DATA).
     * Must happen AFTER send_response since the response reads from RESPONSE TLS. */
    oxphp_bridge_fiber_drop_ctx(fiber->fiber_id);

    /* Do NOT destroy fiber context — the looping coroutine keeps the C stack
     * alive for reuse. zend_fiber_destroy_context is only called in
     * scheduler_destroy (final cleanup). */

    /* Remove from active list */
    if (fiber->prev) fiber->prev->next = fiber->next;
    else sched->fibers_head = fiber->next;
    if (fiber->next) fiber->next->prev = fiber->prev;
    else sched->fibers_tail = fiber->prev;
    sched->fiber_count--;

    /* Return to free list. Clearing the suspension on the way out is what makes
     * "a fiber outside a live wait holds no descriptor pointer" true of every
     * fiber rather than only of the ones that reach the reuse path. */
    oxphp_fiber_clear_suspend(fiber);
    fiber->next = sched->free_list;
    sched->free_list = fiber;
}

/* ─── Descriptor readiness for IO_WAIT-suspended fibers ──
 *
 * Reads whatever the scheduler's epoll instance has to report without waiting,
 * and hands back the fibers that may run again — either because a descriptor
 * they wait on is ready or because their deadline elapsed. The
 * caller resumes them with its own resume/finalize pair, which is the only
 * part that differs between the HTTP and async-task schedulers.
 *
 * Collected fibers stay marked IO_WAIT: the caller clears the mark one fiber
 * at a time, immediately before resuming that fiber. Clearing the whole batch
 * here would leave the rest of it neither parked nor resumed should a resume
 * ever fail to return, and the batch is the one place in the tick where the
 * list is read before the resumes rather than between them.
 *
 * Readiness is checked from the tick rather than driven by an event loop for
 * the same reason sleep timers are: the worker already runs a tick loop, and
 * a suspended fiber is only resumable from the thread that owns it. POLLHUP and
 * POLLERR count as ready for a waiter that asked for them — the delegated
 * caller has to observe EOF and socket errors itself, exactly as it would
 * without a hook — and are muted for one that did not; see
 * oxphp_io_entry_ready(). */

/* Stored in the timer's registration so a wake-up from it can be told apart
 * from a fiber's descriptor without a lookup. Its address is the whole value;
 * the fields are never read. */
static struct oxphp_io_owner oxphp_io_timer_marker;

/* Create this scheduler's epoll instance and its periodic timer on first use.
 * Returns false if either could not be made, which every caller reads as
 * "cannot park": the hooks then delegate and the server keeps working, blocking
 * the way it did before the hooks existed. */
static bool oxphp_io_ensure_epoll(oxphp_fiber_scheduler *sched) {
    if (sched->epfd >= 0) return true;

    int epfd = epoll_create1(EPOLL_CLOEXEC);
    if (epfd < 0) return false;

    int tfd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
    if (tfd < 0) {
        close(epfd);
        return false;
    }

    struct epoll_event ev = {
        .events = EPOLLIN,
        .data = { .ptr = &oxphp_io_timer_marker },
    };
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, tfd, &ev) < 0) {
        close(tfd);
        close(epfd);
        return false;
    }

    sched->epfd = epfd;
    sched->timer_fd = tfd;
    return true;
}

/* ── Which registration a descriptor currently belongs to ──
 *
 * epoll_ctl(EPOLL_CTL_DEL) names a descriptor, and nothing in the call ties the
 * removal to the registration that made it. That is a hole wherever a descriptor
 * number outlives the registration built on it: a fiber parks on fd 7; another
 * fiber closes that stream, which drops the registration inside the kernel with
 * no word to us; the number is handed to a fresh connection whose fiber parks and
 * registers it; and now the first fiber's unpark removes a registration that
 * belongs to someone else, leaving an uninvolved fiber to wait out its deadline
 * with no event that can ever arrive.
 *
 * This table is what a removal is checked against. It mirrors the instance's own
 * contents — one entry per registered descriptor, keyed by the descriptor — and
 * records the identity of the registration that made it, so a removal that is no
 * longer ours is recognised and skipped. The identity is the owner record handed
 * to epoll as the event data: one per (fiber, descriptor), living in the parked
 * fiber's own frame, so no two registrations alive at the same moment can share
 * one address.
 *
 * Open addressing with linear probing and no tombstones, because the traffic is
 * one insert and one erase per descriptor per wait, in bursts the width of a
 * stream_select(). Keyed on the descriptor because that is what epoll keys on,
 * and grown rather than indexed directly by descriptor number: a container is
 * free to hand out a descriptor limit in the millions, and an array of that width
 * per worker thread would dwarf what it protects. */

struct oxphp_io_reg {
    int fd;                          /* -1 marks a free slot; descriptors are never negative */
    struct oxphp_io_owner *owner;
};

static inline uint32_t oxphp_io_reg_hash(int fd) {
    /* Descriptor numbers are dense and small, which linear probing handles
     * badly on its own — consecutive keys pile into consecutive slots. The
     * multiply spreads them before the mask. */
    return (uint32_t) fd * 2654435761u;
}

/* Place a key in a table known to have room. Returns true when it took a free
 * slot, false when it overwrote the entry already held for that descriptor —
 * which is the stale case above, a registration the kernel dropped without us
 * hearing about it, now taken over by whoever registered the number next. */
static bool oxphp_io_reg_insert(struct oxphp_io_reg *slots, uint32_t mask, int fd,
                                struct oxphp_io_owner *owner) {
    uint32_t i = oxphp_io_reg_hash(fd) & mask;
    while (slots[i].fd != -1 && slots[i].fd != fd) {
        i = (i + 1) & mask;
    }
    bool fresh = slots[i].fd == -1;
    slots[i].fd = fd;
    slots[i].owner = owner;
    return fresh;
}

/* Make room for one more entry, allocating on first use and doubling at half
 * load. Half is the point where linear probing stops degrading; the table is
 * small enough (16 bytes an entry) that trading space for that is free. */
static bool oxphp_io_reg_reserve(oxphp_fiber_scheduler *sched) {
    uint32_t cap = sched->reg_slots != NULL ? sched->reg_mask + 1 : 0;
    if (cap != 0 && (sched->reg_count + 1) * 2 <= cap) return true;

    uint32_t new_cap = cap != 0 ? cap * 2 : 64;
    struct oxphp_io_reg *slots = malloc((size_t) new_cap * sizeof(*slots));
    if (slots == NULL) return false;
    for (uint32_t i = 0; i < new_cap; i++) {
        slots[i].fd = -1;
        slots[i].owner = NULL;
    }

    for (uint32_t i = 0; i < cap; i++) {
        if (sched->reg_slots[i].fd != -1) {
            oxphp_io_reg_insert(slots, new_cap - 1, sched->reg_slots[i].fd,
                                sched->reg_slots[i].owner);
        }
    }

    free(sched->reg_slots);
    sched->reg_slots = slots;
    sched->reg_mask = new_cap - 1;
    return true;
}

static bool oxphp_io_reg_put(oxphp_fiber_scheduler *sched, int fd,
                             struct oxphp_io_owner *owner) {
    if (!oxphp_io_reg_reserve(sched)) return false;
    if (oxphp_io_reg_insert(sched->reg_slots, sched->reg_mask, fd, owner)) {
        sched->reg_count++;
    }
    return true;
}

static struct oxphp_io_reg *oxphp_io_reg_find(oxphp_fiber_scheduler *sched, int fd) {
    if (sched->reg_slots == NULL) return NULL;

    uint32_t i = oxphp_io_reg_hash(fd) & sched->reg_mask;
    for (uint32_t probe = 0; probe <= sched->reg_mask; probe++) {
        struct oxphp_io_reg *slot = &sched->reg_slots[(i + probe) & sched->reg_mask];
        if (slot->fd == fd) return slot;
        /* A free slot ends the probe: nothing inserted after it could have
         * passed through, so the key is not in the table. This is what the
         * backward shift below preserves. */
        if (slot->fd == -1) return NULL;
    }
    return NULL;
}

static void oxphp_io_reg_erase(oxphp_fiber_scheduler *sched, struct oxphp_io_reg *slot) {
    /* Linear probing without tombstones: clearing a slot opens a hole that a
     * later key may have probed past, and a lookup would then stop at the hole
     * and miss it. Each such key is pulled back into the hole, which moves the
     * hole along, until a slot is reached that was already free. */
    uint32_t mask = sched->reg_mask;
    uint32_t hole = (uint32_t) (slot - sched->reg_slots);

    sched->reg_slots[hole].fd = -1;
    sched->reg_slots[hole].owner = NULL;
    sched->reg_count--;

    for (uint32_t i = (hole + 1) & mask; sched->reg_slots[i].fd != -1; i = (i + 1) & mask) {
        uint32_t home = oxphp_io_reg_hash(sched->reg_slots[i].fd) & mask;
        if (((i - home) & mask) < ((i - hole) & mask)) continue;
        sched->reg_slots[hole] = sched->reg_slots[i];
        sched->reg_slots[i].fd = -1;
        sched->reg_slots[i].owner = NULL;
        hole = i;
    }
}

/* Stop watching a descriptor — but only if the registration on it is still the
 * one this owner made. A mismatch, or no entry at all, means the number has
 * moved on to someone else since; removing it then is the hole this table
 * exists to close, and doing nothing is exactly right, because whatever we
 * registered is already gone. */
static void oxphp_io_drop_registration(oxphp_fiber_scheduler *sched, int fd,
                                       struct oxphp_io_owner *owner) {
    struct oxphp_io_reg *slot = oxphp_io_reg_find(sched, fd);
    if (slot == NULL || slot->owner != owner) return;

    epoll_ctl(sched->epfd, EPOLL_CTL_DEL, fd, NULL);
    oxphp_io_reg_erase(sched, slot);
}

/* Undo the registrations a park made before it had to give up. */
static void oxphp_io_park_rollback(oxphp_fiber_scheduler *sched, struct pollfd *fds,
                                   struct oxphp_io_owner *owners, uint32_t upto) {
    for (uint32_t j = 0; j < upto; j++) {
        if (fds[j].fd < 0) continue;
        oxphp_io_drop_registration(sched, fds[j].fd, &owners[j]);
    }
}

/* Register every descriptor a fiber is about to wait on. Any refusal means the
 * fiber must not park at all:
 *
 *   - EPERM is a descriptor epoll declines to watch, a regular file being the
 *     common one. Those are always ready as far as a wait is concerned, so the
 *     caller has to go on to its delegate rather than wait for an event that
 *     cannot arrive.
 *   - EEXIST is another fiber already waiting on this descriptor. An epoll
 *     instance holds one registration per descriptor, so there is no way to
 *     route the wake-up to both, and the second waiter blocks its own thread
 *     rather than quietly taking readiness meant for the first. Two fibers
 *     reading one *stream* no longer reach here at all — the claim below keeps
 *     them apart, and this is what remains: the same descriptor arrived at by two
 *     routes, such as a dup() or one stream read while another selects on it.
 *   - a table that cannot grow. Registering without recording who registered it
 *     would leave the removal unverifiable, which is the one thing the table is
 *     for, so the park is declined instead of being left untracked.
 *
 * A refusal partway through rolls back what was already registered, so a
 * declined park leaves the instance exactly as it found it.
 *
 * Entries whose descriptor is negated are skipped: those were muted by an
 * earlier wait in the same call (see oxphp_io_collect_ready), and re-registering
 * one would put back the condition that muted it. A set where every entry is
 * muted registers nothing and simply waits out its deadline. */
bool oxphp_io_park(oxphp_request_fiber *fiber, struct pollfd *fds,
                   struct oxphp_io_owner *owners, uint32_t nfds) {
    oxphp_fiber_scheduler *sched = fiber->owner_sched;
    if (sched == NULL || !oxphp_io_ensure_epoll(sched)) return false;

    for (uint32_t i = 0; i < nfds; i++) {
        if (fds[i].fd < 0) continue;

        struct epoll_event ev = {
            .events = 0,
            .data = { .ptr = &owners[i] },
        };
        if (fds[i].events & POLLIN)  ev.events |= EPOLLIN;
        if (fds[i].events & POLLOUT) ev.events |= EPOLLOUT;
        if (fds[i].events & POLLPRI) ev.events |= EPOLLPRI;

        if (epoll_ctl(sched->epfd, EPOLL_CTL_ADD, fds[i].fd, &ev) < 0) {
            oxphp_io_park_rollback(sched, fds, owners, i);
            return false;
        }
        if (!oxphp_io_reg_put(sched, fds[i].fd, &owners[i])) {
            epoll_ctl(sched->epfd, EPOLL_CTL_DEL, fds[i].fd, NULL);
            oxphp_io_park_rollback(sched, fds, owners, i);
            return false;
        }
    }
    return true;
}

/* Stop watching a fiber's descriptors. Muted entries hold their descriptor
 * negated; it is recovered so the lookup can be made, and the lookup then
 * declines the removal of its own accord — muting already took the registration
 * out, so the table no longer names this owner for that descriptor. */
void oxphp_io_unpark(oxphp_request_fiber *fiber) {
    oxphp_fiber_scheduler *sched = fiber->owner_sched;
    if (sched == NULL || sched->epfd < 0) return;

    for (uint32_t i = 0; i < fiber->suspend_data.io.nfds; i++) {
        int fd = fiber->suspend_data.io.fds[i].fd;
        oxphp_io_drop_registration(sched, fd < 0 ? -1 - fd : fd,
                                   &fiber->suspend_data.io.owners[i]);
    }
}

/* ─── Which fiber a connection belongs to ─────────────────
 *
 * A client protocol on a socket is a sequence of exchanges — write a command,
 * read the answer — and the connection carries no marker for where one ends.
 * Once a hooked read parks a fiber halfway through one, the worker runs another
 * fiber on the same PHP context, and an application that opened its database or
 * cache client when the worker booted hands both fibers the same connection. The
 * second fiber's command then lands in the middle of the first one's exchange.
 * Clients answer that differently and neither answer is usable: mysqlnd tracks
 * its connection state and refuses the command outright, while phpredis has no
 * such check, so both commands reach the server and each fiber reads the other's
 * reply — one request's data returned to another, with no error raised.
 *
 * So a fiber claims a connection before using it, and a fiber that meets a
 * connection another one has claimed waits for it to be given up instead of
 * joining in. The claim is dropped when the owning fiber's request or task ends,
 * not when its read returns: the exchange boundary cannot be seen from here, and
 * the end of the request is the first moment certainly past it. Holding it that
 * long over-serializes — a fiber that queried once keeps the connection until its
 * request is done — and the cost of that is the hook's benefit on a shared
 * connection, nothing else. Such a connection then carries the throughput it had
 * before the hook existed, which is the intended trade: the gain stays where
 * connections are not shared, and correctness does not depend on which of those
 * an application does.
 *
 * The key is a `void *` because a connection is named at two levels and both need
 * claiming. The socket ops name a `php_stream`, which is what keeps the bytes on
 * the wire in order. The database clients need one level up: mysqlnd refuses a
 * reentrant command from its own connection state, before any I/O, so no
 * stream-level guard can be reached at all — for those, the hooked client entry
 * points claim whatever names the connection there (the driver's own connection
 * handle where it can be reached, the client object otherwise). Keyed on the
 * stream rather than the descriptor for the same reason a claim is not keyed on
 * the client object where the handle is available: a descriptor number, like a
 * PHP object, names a connection for less time than the connection lasts.
 *
 * No key outlives what it names by construction, so all of them rely on the entry
 * going away in time. A closed stream erases its entry (the close op is hooked for
 * that alone). A connection has no equally cheap hook, so one closed mid-request
 * can leave an entry that a later connection at the same address inherits; the
 * consequence is one bounded wait that ends in the unguarded behaviour, and every
 * entry is gone by the end of the request either way.
 *
 * Open addressing with linear probing and no tombstones, exactly as
 * oxphp_io_reg_* above: the same traffic shape, and one idiom to read rather
 * than two. Thread-local rather than per scheduler because fibers only ever
 * multiplex within a thread, so one table covers the request fibers and the task
 * fibers on it without either scheduler having to know about the other. */

struct oxphp_claim {
    void *key;                       /* NULL marks a free slot */
    oxphp_request_fiber *owner;
};

static __thread struct oxphp_claim *oxphp_claim_slots = NULL;
static __thread uint32_t oxphp_claim_mask = 0;
static __thread uint32_t oxphp_claim_count = 0;

static inline uint32_t oxphp_claim_hash(const void *key) {
    /* Heap addresses are allocator-aligned, so their low bits say almost
     * nothing; dropping them before the multiply is what keeps a run of
     * consecutive allocations from probing as a run. */
    return (uint32_t) ((((uintptr_t) key) >> 4) * 2654435761u);
}

/* Place a key in a table known to have room. Returns true when it took a free
 * slot, false when it overwrote the entry already held for that stream. */
static bool oxphp_claim_insert(struct oxphp_claim *slots, uint32_t mask,
                               void *key, oxphp_request_fiber *owner) {
    uint32_t i = oxphp_claim_hash(key) & mask;
    while (slots[i].key != NULL && slots[i].key != key) {
        i = (i + 1) & mask;
    }
    bool fresh = slots[i].key == NULL;
    slots[i].key = key;
    slots[i].owner = owner;
    return fresh;
}

/* Make room for one more entry, allocating on first use and doubling at half
 * load — the point where linear probing stops degrading. */
static bool oxphp_claim_reserve(void) {
    uint32_t cap = oxphp_claim_slots != NULL ? oxphp_claim_mask + 1 : 0;
    if (cap != 0 && (oxphp_claim_count + 1) * 2 <= cap) return true;

    uint32_t new_cap = cap != 0 ? cap * 2 : 16;
    struct oxphp_claim *slots = malloc((size_t) new_cap * sizeof(*slots));
    if (slots == NULL) return false;
    for (uint32_t i = 0; i < new_cap; i++) {
        slots[i].key = NULL;
        slots[i].owner = NULL;
    }

    for (uint32_t i = 0; i < cap; i++) {
        if (oxphp_claim_slots[i].key != NULL) {
            oxphp_claim_insert(slots, new_cap - 1, oxphp_claim_slots[i].key,
                               oxphp_claim_slots[i].owner);
        }
    }

    free(oxphp_claim_slots);
    oxphp_claim_slots = slots;
    oxphp_claim_mask = new_cap - 1;
    return true;
}

static struct oxphp_claim *oxphp_claim_find(void *key) {
    if (oxphp_claim_slots == NULL) return NULL;

    uint32_t i = oxphp_claim_hash(key) & oxphp_claim_mask;
    for (uint32_t probe = 0; probe <= oxphp_claim_mask; probe++) {
        struct oxphp_claim *slot = &oxphp_claim_slots[(i + probe) & oxphp_claim_mask];
        if (slot->key == key) return slot;
        /* A free slot ends the probe: nothing inserted after it could have
         * passed through, so the key is not in the table. */
        if (slot->key == NULL) return NULL;
    }
    return NULL;
}

/* Clear one slot, pulling back the keys that probed past it — the same backward
 * shift oxphp_io_reg_erase does, and for the same reason: a hole left in place
 * would end a probe that should have carried on through it. */
static void oxphp_claim_erase(struct oxphp_claim *slot) {
    uint32_t mask = oxphp_claim_mask;
    uint32_t hole = (uint32_t) (slot - oxphp_claim_slots);

    oxphp_claim_slots[hole].key = NULL;
    oxphp_claim_slots[hole].owner = NULL;
    oxphp_claim_count--;

    for (uint32_t i = (hole + 1) & mask; oxphp_claim_slots[i].key != NULL;
         i = (i + 1) & mask) {
        uint32_t home = oxphp_claim_hash(oxphp_claim_slots[i].key) & mask;
        if (((i - home) & mask) < ((i - hole) & mask)) continue;
        oxphp_claim_slots[hole] = oxphp_claim_slots[i];
        oxphp_claim_slots[i].key = NULL;
        oxphp_claim_slots[i].owner = NULL;
        hole = i;
    }
}

oxphp_request_fiber *oxphp_claim_owner(void *key) {
    struct oxphp_claim *slot = oxphp_claim_find(key);
    return slot != NULL ? slot->owner : NULL;
}

bool oxphp_claim_acquire(void *key, oxphp_request_fiber *owner) {
    struct oxphp_claim *slot = oxphp_claim_find(key);
    if (slot != NULL) {
        /* Already recorded: either this fiber's own claim being renewed, or one
         * the previous owner released and left the entry of. The caller has
         * established which — this is not the place that decides. */
        slot->owner = owner;
        return true;
    }

    if (!oxphp_claim_reserve()) return false;
    if (oxphp_claim_insert(oxphp_claim_slots, oxphp_claim_mask, key, owner)) {
        oxphp_claim_count++;
    }
    return true;
}

void oxphp_claim_forget(void *key) {
    struct oxphp_claim *slot = oxphp_claim_find(key);
    if (slot != NULL) oxphp_claim_erase(slot);
}

/* Give up every stream this fiber holds. Called where a request or task ends,
 * which is the release point the claim is defined against. */
static void oxphp_claim_release_fiber(const oxphp_request_fiber *fiber) {
    if (oxphp_claim_slots == NULL || oxphp_claim_count == 0) return;

    uint32_t i = 0;
    while (i <= oxphp_claim_mask) {
        if (oxphp_claim_slots[i].key != NULL && oxphp_claim_slots[i].owner == fiber) {
            /* The erase pulls a later key back into this slot, so the slot has
             * to be looked at again rather than stepped over. Each pass through
             * here removes one entry, so this terminates. */
            oxphp_claim_erase(&oxphp_claim_slots[i]);
            continue;
        }
        i++;
    }
}

/* Drop the table once it holds nothing. Called from scheduler teardown, which is
 * where a thread stops having fibers — and a worker thread does come and go
 * under dynamic scaling, so the table must not simply be left behind. Guarded on
 * being empty because the table is thread-local while teardown is per scheduler:
 * a thread carrying both would otherwise have one scheduler's exit take the
 * other's claims with it. */
static void oxphp_claim_reset_if_empty(void) {
    if (oxphp_claim_count != 0) return;

    free(oxphp_claim_slots);
    oxphp_claim_slots = NULL;
    oxphp_claim_mask = 0;
}

/* Whether this scheduler has anything parked on a descriptor at all. */
static bool oxphp_io_any_parked(const oxphp_fiber_scheduler *sched) {
    for (const oxphp_request_fiber *fiber = sched->fibers_head; fiber;
         fiber = fiber->next) {
        if (!fiber->completed && fiber->suspend_reason == OXPHP_SUSPEND_IO_WAIT) {
            return true;
        }
    }
    return false;
}

/* Wait up to `ns` nanoseconds for one of the descriptors this scheduler has
 * fibers parked on, and report whether there was anything to wait for.
 *
 * This exists so an idle worker sleeps *on the sockets* instead of sleeping a
 * fixed interval and noticing readiness on the following tick. With a fixed
 * backoff every socket round trip pays up to a full interval of latency, which
 * on a chatty protocol is the dominant cost of the hook; waiting on the
 * descriptors ends the pause the moment the peer answers. The interval is
 * unchanged, so a newly queued request waits no longer than it did before. */
bool oxphp_scheduler_io_backoff(oxphp_fiber_scheduler *sched, int64_t ns) {
    /* Zero is rejected alongside negative: it would arm nothing, and a wait with
     * nothing to bound it blocks until a socket happens to fire — the worker
     * would stop accepting requests and stop examining deadlines for as long as
     * that took. No caller passes zero today; the guard is what keeps that from
     * becoming a silent hang if one ever does. */
    if (ns <= 0 || sched->epfd < 0) return false;

    /* Nothing parked means there is nothing to wait on and the caller must sleep
     * its own interval. Waiting on the timer alone would put a syscall where a
     * sleep belongs and, worse, would report a wait to a caller that is meant to
     * hear it did not happen. */
    if (!oxphp_io_any_parked(sched)) return false;

    /* Anything the timer left behind goes before the next wait is armed, or that
     * expiry would answer the wait at once. Read first and arm second, never the
     * other way round: arming starts the interval from now, so after it there is
     * nothing stale to consume — whereas a read placed after the arm could eat
     * the very expiry the wait is waiting for and block for good. */
    uint64_t ticks;
    ssize_t drained = read(sched->timer_fd, &ticks, sizeof(ticks));
    (void) drained;

    /* The timer is what bounds the wait, which lets epoll_wait() block outright
     * rather than take the millisecond timeout it offers — a resolution that
     * would round the worker's 100µs interval either to nothing or to ten times
     * itself.
     *
     * One-shot, re-armed on every call, so the pause is the interval the caller
     * asked for rather than whatever is left of a free-running period. A
     * periodic timer keeps expiring while the worker is busy elsewhere, and
     * those expiries cut the next pause short — on average to half the interval,
     * and to nothing at all once one turn of the caller's loop costs more than
     * one period, which is the busy case this is meant to damp. */
    struct itimerspec its = {
        .it_interval = { .tv_sec = 0, .tv_nsec = 0 },
        .it_value    = { .tv_sec = (time_t)(ns / 1000000000),
                         .tv_nsec = (long)(ns % 1000000000) },
    };
    if (timerfd_settime(sched->timer_fd, 0, &its, NULL) < 0) return false;

    /* One slot: this waits, it does not resolve. Readiness and deadlines are
     * decided by oxphp_io_collect_ready(), and the registrations are
     * level-triggered, so whatever fired is still there for it to find.
     *
     * A signal cuts the pause short; unlike php_sock_stream_wait_for_data this
     * does not retry on EINTR, because there is nothing to preserve — the caller
     * loops, and an early return costs it one extra turn round that loop, not a
     * spin: a profiler sampling at 100 Hz skips one pause per 10 ms.
     *
     * Any other failure is reported as "did not wait" so the caller sleeps its
     * own interval instead. Claiming to have waited when the call failed would
     * turn a persistent error into a busy loop at full CPU with nothing in the
     * log to explain it. */
    struct epoll_event ev;
    if (epoll_wait(sched->epfd, &ev, 1, -1) < 0 && errno != EINTR) {
        return false;
    }
    return true;
}

/* Whether an entry reports something its waiter asked for.
 *
 * A hangup, a socket error and an invalid descriptor are reported whether or not
 * they were asked for — epoll and poll agree on that, and those three are the
 * only conditions either adds; everything else appears solely because the caller
 * asked for it. An invalid descriptor always ends a wait:
 * the descriptor is unusable and the delegated caller has to find that out for
 * itself. The other two end it only when the waiter said they would act on
 * them, because that is the rule PHP's own multiplexed wait follows — it maps a
 * hangup onto the read set alone and a socket error onto the read and write
 * sets, never onto the exception set. A waiter released on a bit its delegate
 * then declines to report would call the delegate, be told nothing happened,
 * and park again, which for a hangup is a busy loop with no end. */
static inline bool oxphp_io_entry_ready(const struct pollfd *pfd) {
    return (pfd->revents & (pfd->events | POLLNVAL)) != 0;
}

/* Stop watching one entry for the rest of its suspension, after it reported
 * something and none of it was wanted. What is reported uninvited is POLLERR or
 * POLLHUP, and neither ever clears — the descriptor is finished. Left in place
 * it would answer every wait instantly and the idle backoff would stop sleeping
 * at all, at full CPU, for as long as the waiter holds on. Nothing is lost:
 * whatever the waiter did ask for could only arrive on a descriptor still
 * capable of delivering it.
 *
 * The descriptor is negated rather than overwritten so it stays recoverable from
 * the entry — deregistering needs it, and so does anyone reading the set. */
static void oxphp_io_mute_entry(oxphp_fiber_scheduler *sched, struct pollfd *entry,
                                struct oxphp_io_owner *owner) {
    oxphp_io_drop_registration(sched, entry->fd, owner);
    entry->fd = -1 - entry->fd;
}

static uint32_t oxphp_io_collect_ready(oxphp_fiber_scheduler *sched,
                                       oxphp_request_fiber **out, uint32_t max) {
    /* No instance means nothing has ever parked here, which may skip the
     * readiness half but never the deadline pass: a fiber whose timeout has
     * already elapsed must be released whether or not anyone watched its
     * descriptor. Only the readiness half is conditional. */
    if (sched->epfd >= 0) {
        /* Several fibers can be ready at once and one fiber can hold more
         * descriptors than this, so the batch can overflow. Nothing is lost: the
         * registrations are level-triggered, so whatever did not fit is reported
         * again on the next tick. */
        struct epoll_event evs[OXPHP_MAX_FIBERS];
        int n = epoll_wait(sched->epfd, evs, OXPHP_MAX_FIBERS, 0);

        if (n < 0 && errno != EINTR) {
            /* A descriptor that has been closed or was never valid is reported
             * per registration, so this is the whole-call failure (EFAULT,
             * EINVAL): nothing was examined and every waiter would otherwise
             * stay parked forever. Release them all rather than strand them, by
             * marking each entry with conditions the interest mask always lets
             * through — POLLNVAL alongside POLLERR, or a waiter that asked only
             * about out-of-band data would be left behind.
             *
             * Be clear about what that costs: each released fiber goes on to its
             * delegate, which waits on its own descriptor again, usually
             * succeeds, and blocks the worker thread for the socket's full
             * timeout — the very thing the hook exists to avoid. The hook
             * degrades to native behaviour, which is survivable, but it is
             * silent otherwise, so say it once. Once for the process, not once
             * per worker: every thread that hooks sockets would otherwise repeat
             * the same line, and the failure is a property of the platform, not
             * of the thread that noticed it. The exchange is what makes "once"
             * true when two workers notice together. */
            static atomic_bool reported = false;
            if (!atomic_exchange(&reported, true)) {
                php_log_err("oxphp: waiting on parked socket descriptors failed; hooked "
                            "reads fall back to blocking the worker thread until it recovers");
            }
            for (oxphp_request_fiber *fiber = sched->fibers_head; fiber;
                 fiber = fiber->next) {
                if (fiber->completed || fiber->suspend_reason != OXPHP_SUSPEND_IO_WAIT) {
                    continue;
                }
                for (uint32_t i = 0; i < fiber->suspend_data.io.nfds; i++) {
                    fiber->suspend_data.io.fds[i].revents = POLLERR | POLLNVAL;
                }
            }
        }

        /* Scatter readiness back into each fiber's own array: that is where the
         * suspended code reads it, and a fiber waiting on several descriptors
         * needs to know which of them fired, not merely that one did. The
         * registration carries the fiber and the index because the event itself
         * does not carry the descriptor. The timer's own registration is skipped
         * — the idle backoff is what consumes that one. */
        for (int i = 0; i < n; i++) {
            if (evs[i].data.ptr == &oxphp_io_timer_marker) continue;

            struct oxphp_io_owner *owner = (struct oxphp_io_owner *) evs[i].data.ptr;
            oxphp_request_fiber *fiber = owner->fiber;
            if (fiber->suspend_reason != OXPHP_SUSPEND_IO_WAIT) continue;

            struct pollfd *entry = &fiber->suspend_data.io.fds[owner->idx];
            /* Already muted, and only reachable if the removal did not take.
             * Negating twice would restore the descriptor and un-mute it. */
            if (entry->fd < 0) continue;

            short revents = 0;
            if (evs[i].events & EPOLLIN)  revents |= POLLIN;
            if (evs[i].events & EPOLLOUT) revents |= POLLOUT;
            if (evs[i].events & EPOLLPRI) revents |= POLLPRI;
            if (evs[i].events & EPOLLERR) revents |= POLLERR;
            if (evs[i].events & EPOLLHUP) revents |= POLLHUP;
            entry->revents = revents;

            if (revents != 0 && !oxphp_io_entry_ready(entry)) {
                oxphp_io_mute_entry(sched, entry, owner);
            }
        }
    }

    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t now_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;

    uint32_t ready = 0;
    for (oxphp_request_fiber *fiber = sched->fibers_head; fiber && ready < max;
         fiber = fiber->next) {
        if (fiber->completed || fiber->suspend_reason != OXPHP_SUSPEND_IO_WAIT) {
            continue;
        }
        bool any = false;
        for (uint32_t i = 0; i < fiber->suspend_data.io.nfds; i++) {
            if (oxphp_io_entry_ready(&fiber->suspend_data.io.fds[i])) {
                any = true;
                break;
            }
        }
        bool expired = !any
                       && fiber->suspend_data.io.deadline_ns != 0
                       && now_ns >= fiber->suspend_data.io.deadline_ns;
        if (!any && !expired) continue;

        fiber->suspend_data.io.expired = expired;
        out[ready++] = fiber;
    }
    return ready;
}

/* ─── Event Loop Tick ──────────────────────────────────── */

int oxphp_scheduler_tick(oxphp_fiber_scheduler *sched) {
    int work_done = 0;

    /* 1. Check for new requests (non-blocking) */
    while (sched->fiber_count < OXPHP_MAX_FIBERS) {
        int rc = oxphp_bridge_worker_try_recv();
        if (rc == -1) return -1; /* shutdown */
        if (rc == 1) break;     /* empty */

        /* Got a request — prepare TLS and create fiber.
         *
         * IMPORTANT: Do NOT call oxphp_soft_reset() here! It resets global
         * PHP thread state (SG headers, PG superglobals, output buffers)
         * which would clobber suspended fibers' saved state.
         *
         * Instead, prepare_request handles Rust TLS setup, and
         * oxphp_fiber_init_request_state() does a targeted per-fiber init
         * (fresh superglobals, clean SAPI headers) without touching global OB. */
        oxphp_bridge_prepare_request();
        oxphp_fiber_init_request_state();

        /* Increment counter at request START (mirror of fast path in
         * oxphp_sapi.c). Keeps sched->total_requests_done as a mirror of
         * bridge state, used by exit-condition checks downstream. */
        sched->total_requests_done = oxphp_bridge_increment_requests_done();

        /* Create or reuse fiber (captures the request's cancel cell) */
        oxphp_request_fiber *fiber = oxphp_scheduler_create_fiber(
            sched, sched->shared_fci, sched->shared_fcc);
        if (!fiber) break;

        /* Fresh or recycled, a new request is always a start, never a resume. */
        oxphp_scheduler_start_fiber(sched, fiber);

        /* Check if it completed immediately (no suspend) */
        if (fiber->completed) {
            oxphp_scheduler_finalize_fiber(sched, fiber);
        }

        work_done = 1;
    }

    /* 2. Check completed async awaits (or expired per-call await deadlines). */
    {
        /* Snapshot the clock once per tick, not per fiber. */
        struct timespec ts;
        clock_gettime(CLOCK_MONOTONIC, &ts);
        uint64_t now_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;

        oxphp_request_fiber *fiber = sched->fibers_head;
        while (fiber) {
            oxphp_request_fiber *next = fiber->next; /* save — finalize may unlink */

            if (fiber->suspend_reason == OXPHP_SUSPEND_AWAIT) {
                /* Resume when the awaited promise is ready or the fiber was
                 * cancelled; otherwise unwind the await once its per-call
                 * deadline elapses. Mirrors the task scheduler so a worker-mode
                 * request fiber honours await($p, timeout) instead of blocking
                 * until the promise settles — a ready result / cancellation
                 * takes precedence over the deadline. */
                bool ready = fiber->cancel_requested
                             || oxphp_bridge_await_poll(fiber->suspend_data.promise_id);
                bool deadline = !ready && fiber->await_deadline_ns != 0
                                && now_ns >= fiber->await_deadline_ns;
                if (deadline) {
                    fiber->timed_out = true;
                }
                if (ready || deadline) {
                    oxphp_fiber_clear_suspend(fiber);
                    oxphp_scheduler_resume_fiber(sched, fiber, NULL);

                    if (fiber->completed) {
                        oxphp_scheduler_finalize_fiber(sched, fiber);
                    }
                    work_done = 1;
                }
            }
            /* TODO: AWAIT_ALL, AWAIT_ANY — similar pattern */

            fiber = next;
        }
    }

    /* 2b. Drain sweep, two phases. Soft phase (is_draining): unwind only
     * fibers whose request holds an OPEN stream — their response never
     * finishes on its own, and the client is built to reconnect. A streaming
     * request that already called oxphp_finish_request() is not open: its
     * response is complete and its remaining background work is ordinary. An
     * ordinary request
     * suspended in a short await/sleep is left alone; it resumes normally and
     * gets the whole drain window to finish with a real response. Hard phase
     * (is_drain_hard, latched at the drain deadline): unwind EVERY suspended
     * fiber — the broadcast vm_interrupt kick only reaches running code, so
     * this sweep is the only thing that can end a suspended straggler.
     * For each victim: mark drain_kill and resume; the suspend point sees the
     * mark and bails uncatchably via zend_error_noreturn — a user try/catch
     * cannot swallow it, and registered shutdown functions still run. Covers
     * SLEEP and every AWAIT variant, and every fiber on the worker — not just
     * the one a single per-thread vm_interrupt would reach. The CAS on the
     * cell is observability only (response cancel_reason, metrics); the kill
     * must not depend on it — the cell can already hold ClientAbort/Timeout,
     * and a resume that fell through would block forever in await_dispatch on
     * an unsettled promise. */
    if (oxphp_bridge_is_draining()) {
        bool drain_hard = oxphp_bridge_is_drain_hard();
        oxphp_request_fiber *fiber = sched->fibers_head;
        while (fiber) {
            oxphp_request_fiber *next = fiber->next;
            bool open_stream = fiber->php_state.bridge_stream_mode
                            && !fiber->php_state.bridge_finished;
            if (!fiber->completed && fiber->suspend_reason != OXPHP_SUSPEND_NONE
                && (drain_hard || open_stream)) {
                oxphp_bridge_set_cancel_reason_at(fiber->request_cancel_ptr,
                                                  OXPHP_CANCEL_SHUTDOWN);
                fiber->drain_kill = true;
                oxphp_fiber_clear_suspend(fiber);
                oxphp_scheduler_resume_fiber(sched, fiber, NULL);
                if (fiber->completed) {
                    oxphp_scheduler_finalize_fiber(sched, fiber);
                }
                work_done = 1;
            }
            fiber = next;
        }
    }

    /* 3. Check expired timers */
    {
        uint64_t ready_ids[32];
        uint32_t count = oxphp_bridge_timer_poll(ready_ids, 32);
        for (uint32_t i = 0; i < count; i++) {
            oxphp_request_fiber *fiber = sched->fibers_head;
            while (fiber) {
                oxphp_request_fiber *next = fiber->next;
                if (fiber->suspend_reason == OXPHP_SUSPEND_SLEEP
                    && fiber->suspend_data.timer_id == ready_ids[i]) {
                    oxphp_fiber_clear_suspend(fiber);
                    oxphp_scheduler_resume_fiber(sched, fiber, NULL);
                    if (fiber->completed) {
                        oxphp_scheduler_finalize_fiber(sched, fiber);
                    }
                    work_done = 1;
                    break;
                }
                fiber = next;
            }
        }
    }

    /* 3b. Resume fibers whose descriptor became ready (or whose read/write
     * deadline elapsed). Hooked socket I/O parks a request fiber here instead
     * of blocking the worker thread inside a wait of its own. */
    {
        oxphp_request_fiber *ready[OXPHP_MAX_FIBERS];
        uint32_t count = oxphp_io_collect_ready(sched, ready, OXPHP_MAX_FIBERS);
        for (uint32_t i = 0; i < count; i++) {
            /* Re-check each entry instead of trusting the batch. Every other
             * pass re-reads the list between resumes because a resume runs PHP
             * and a finalize unlinks fibers; nothing today makes one fiber's
             * resume finalize another, and this is what keeps that assumption
             * from being load-bearing here alone. */
            if (ready[i]->completed
                || ready[i]->suspend_reason != OXPHP_SUSPEND_IO_WAIT) {
                continue;
            }
            oxphp_fiber_clear_suspend(ready[i]);
            oxphp_scheduler_resume_fiber(sched, ready[i], NULL);
            if (ready[i]->completed) {
                oxphp_scheduler_finalize_fiber(sched, ready[i]);
            }
            work_done = 1;
        }
    }

    /* 4. Drain orphaned promises of finished fibers, non-blocking. finalize
     * moves a completed request's still-running fire-and-forget promises here
     * instead of block_on'ing on the worker thread; this poll releases each
     * one's frozen captures once its task settles (or its budget expires). */
    oxphp_bridge_poll_deferred_drains();

    return work_done;
}

/* ─── Async-task scheduler ─────────────────────────────────────
 *
 * Drives oxphp_async tasks through scheduler fibers so one async worker
 * thread can hold many tasks in-flight, each suspending at its await /
 * sleep / channel boundary. The Rust driver loop (src/executor/
 * async_pool.rs) calls spawn → tick → poll_completed → release through
 * the bridge forwarders registered at MINIT.
 *
 * Reuses the HTTP fiber struct (oxphp_request_fiber + its task_* fields).
 * Unlike HTTP request fibers, task fibers carry no per-request superglobal /
 * SAPI-header / Rust-TLS state, so their switch wrappers move nothing but the
 * C-stack bounds — concurrent suspended tasks are kept apart by the Zend VM
 * state zend_fiber_switch_context carries per switching frame, which is the
 * same thing that keeps userland Fibers apart.
 * Superglobals and output buffers stay shared on the worker
 * (background tasks are read-only w.r.t. them — same as the previous
 * synchronous model, which never reset superglobals between tasks).
 *
 * Lifecycle: spawn reconstructs the closure into the fiber's owned
 * task_fci/task_fcc and runs it to its first suspend or completion.
 * Completed fibers stay in the active list (flagged `completed`) until
 * the Rust driver drains them: poll_completed returns pointers into the
 * fiber's task_retval / task_exc_*, the driver serialises them, then
 * release tears the fiber down and recycles it. tick only advances
 * suspended fibers whose await/sleep is ready. */

/* Per-thread task scheduler. Async worker threads run exactly one; HTTP
 * worker threads never touch it. Lazily initialised on first spawn. */
static __thread oxphp_fiber_scheduler oxphp_task_sched;
static __thread bool oxphp_task_sched_inited = false;

static void task_fiber_coroutine(zend_fiber_transfer *transfer);
static void oxphp_task_start_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber);
static void oxphp_task_resume_fiber(oxphp_fiber_scheduler *sched,
                                    oxphp_request_fiber *fiber, zval *value);

static inline oxphp_fiber_scheduler *oxphp_task_sched_get(void) {
    if (!oxphp_task_sched_inited) {
        oxphp_scheduler_init(&oxphp_task_sched);
        oxphp_task_sched_inited = true;
    }
    return &oxphp_task_sched;
}

/* Capture a pending PHP exception (EG(exception)) into malloc'd strings on
 * the fiber, then clear it, so the result can be propagated to the awaiter
 * after the task fiber unwinds. */
static void task_capture_exception(oxphp_request_fiber *fiber) {
    zend_object *ex = EG(exception);
    zend_class_entry *ce = ex->ce;
    fiber->task_exc_class = strdup(ZSTR_VAL(ce->name));

    zval rv;
    zval *msg_zv = zend_read_property(ce, ex, "message", sizeof("message") - 1, 1, &rv);
    if (msg_zv && Z_TYPE_P(msg_zv) == IS_STRING) {
        fiber->task_exc_message = strdup(Z_STRVAL_P(msg_zv));
    } else {
        fiber->task_exc_message = strdup("(unknown)");
    }
    zend_clear_exception();
}

/* Capture a fatal error (zend_bailout) into malloc'd strings on the fiber,
 * parsing the message the Rust error callback stashed via
 * oxphp_bridge_capture_fatal before the bailout. */
static void task_capture_fatal(oxphp_request_fiber *fiber) {
    char *fatal_msg = oxphp_bridge_pop_fatal();
    if (fatal_msg && strncmp(fatal_msg, "Uncaught ", 9) == 0) {
        const char *class_start = fatal_msg + 9;
        const char *colon = strchr(class_start, ':');
        if (colon && colon > class_start) {
            fiber->task_exc_class = strndup(class_start, (size_t)(colon - class_start));
            const char *msg_start = colon + 2;
            const char *in_pos = strstr(msg_start, " in ");
            if (in_pos) {
                fiber->task_exc_message = strndup(msg_start, (size_t)(in_pos - msg_start));
            } else {
                fiber->task_exc_message = strdup(msg_start);
            }
        } else {
            fiber->task_exc_class = strdup("Error");
            fiber->task_exc_message = strdup(fatal_msg);
        }
        free(fatal_msg);
    } else if (fatal_msg) {
        fiber->task_exc_class = strdup("Error");
        fiber->task_exc_message = fatal_msg; /* transfer ownership */
    } else {
        fiber->task_exc_class = strdup("Error");
        fiber->task_exc_message = strdup("Fatal error in async closure");
    }
    CG(unclean_shutdown) = 0;
}

/* Looping coroutine for task fibers. Runs the per-task closure (already
 * reconstructed by spawn into task_fci/task_fcc), captures retval/exception,
 * marks completed, and suspends back to the scheduler. When the fiber is
 * recycled for a later task, spawn sets fresh task state and resumes the
 * loop. */
static void task_fiber_coroutine(zend_fiber_transfer *transfer) {
    oxphp_request_fiber *fiber = (oxphp_request_fiber *)EG(current_fiber_context)->kind;
    fiber->scheduler = transfer->context;

    /* Set stack-overflow detection limits ONCE (C stack is reused) */
    int stack_anchor;
    oxphp_fiber_set_stack_limits_from_sp(&stack_anchor, EG(fiber_stack_size));
    fiber->saved_stack_base = EG(stack_base);
    fiber->saved_stack_limit = EG(stack_limit);

    for (;;) {
        oxphp_current_fiber = fiber;

        /* Fresh VM stack per task (cheap emalloc, not mmap) */
        EG(vm_stack) = zend_vm_stack_new_page(ZEND_FIBER_VM_STACK_SIZE, NULL);
        EG(vm_stack_top) = EG(vm_stack)->top;
        EG(vm_stack_end) = EG(vm_stack)->end;
        EG(vm_stack_page_size) = ZEND_FIBER_VM_STACK_SIZE;
        EG(current_execute_data) = NULL;
        /* Start uncoupled from any in-progress trace recording (upstream
         * zend_fiber_execute does the same on coroutine entry). */
        EG(jit_trace_num) = 0;

        ZVAL_UNDEF(&fiber->task_retval);

        zend_try {
            fiber->task_fci.retval = &fiber->task_retval;
            fiber->task_fci.param_count = fiber->task_argc;
            fiber->task_fci.params = fiber->task_args;
            if (zend_call_function(&fiber->task_fci, &fiber->task_fcc) != SUCCESS) {
                fiber->task_exc_class = strdup("RuntimeException");
                fiber->task_exc_message = strdup("Failed to call async closure");
            } else if (EG(exception)) {
                task_capture_exception(fiber);
            }
        } zend_catch {
            task_capture_fatal(fiber);
        } zend_end_try();

        /* The task is over: release the socket streams it claimed, so a sibling
         * task waiting for one of them can go on. Same point, same reasoning as
         * in request_fiber_coroutine. */
        oxphp_claim_release_fiber(fiber);

        oxphp_current_fiber = NULL;

        /* Destroy this task's VM stack (emalloc'd — cheap) */
        zend_vm_stack_destroy();

        /* Mark completed and suspend back to scheduler. The fiber stays in
         * the active list until the driver drains it (poll) and releases it. */
        fiber->completed = true;

        oxphp_fiber_park(fiber);

        /* ── Resumed to run the next task (recycled fiber) ──
         * spawn has reconstructed a fresh closure into task_fci/task_fcc,
         * set task_args/argc, and cleared `completed` before resuming. */
    }
}

/* Switch into a task fiber to run a NEW task — a fresh context whose coroutine
 * begins at its entry, or a recycled one parked at the bottom of its loop.
 * Task-side mirror of oxphp_scheduler_start_fiber; a task carries no HTTP
 * request state, so the C-stack bounds are the only thing installed here. */
static void oxphp_task_start_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber) {
    sched->current = fiber;

    void *saved_stack_base = EG(stack_base);
    void *saved_stack_limit = EG(stack_limit);
    oxphp_fiber_install_stack_limits(fiber);

    oxphp_fiber_enter(fiber, NULL);

    EG(stack_base) = saved_stack_base;
    EG(stack_limit) = saved_stack_limit;
    sched->current = NULL;
}

/* Resume a suspended task fiber. Task-side mirror of
 * oxphp_scheduler_resume_fiber, with no per-request state to re-install. */
static void oxphp_task_resume_fiber(oxphp_fiber_scheduler *sched,
                                    oxphp_request_fiber *fiber, zval *value) {
    sched->current = fiber;
    oxphp_current_fiber = fiber;

    void *saved_stack_base = EG(stack_base);
    void *saved_stack_limit = EG(stack_limit);
    oxphp_fiber_install_stack_limits(fiber);

    oxphp_fiber_enter(fiber, value);

    EG(stack_base) = saved_stack_base;
    EG(stack_limit) = saved_stack_limit;
    oxphp_current_fiber = NULL;
    sched->current = NULL;
}

int64_t oxphp_async_sched_spawn(void *op_array, void *static_vars,
                                void *this_ptr, uint32_t argc, void *args,
                                void *cancel_cell) {
    oxphp_fiber_scheduler *sched = oxphp_task_sched_get();

    if (sched->fiber_count >= OXPHP_MAX_FIBERS) {
        return -1; /* at per-worker capacity */
    }

    oxphp_request_fiber *fiber;
    bool reused = false;
    if (sched->free_list) {
        fiber = sched->free_list;
        sched->free_list = fiber->next;
        reused = true;
    } else {
        fiber = ecalloc(1, sizeof(oxphp_request_fiber));
    }

    if (!reused) {
        if (zend_fiber_init_context(
                &fiber->context,
                (void *)fiber,
                task_fiber_coroutine,
                EG(fiber_stack_size)) != SUCCESS) {
            efree(fiber);
            return -1;
        }
    }

    fiber->fiber_id = sched->next_fiber_id++;
    fiber->owner_sched = sched;
    fiber->task_mode = true;
    fiber->cancel_requested = false;
    fiber->timed_out = false;
    fiber->await_deadline_ns = 0;
    fiber->cancel_cell = (_Atomic(uint8_t) *)cancel_cell;
    fiber->request_cancel_ptr = NULL; /* task fibers carry no HTTP request cell */
    fiber->drain_kill = false;
    oxphp_fiber_clear_suspend(fiber);
    fiber->completed = false;
    fiber->handler_failed = false;
    fiber->consecutive_errors = 0;
    fiber->task_args = NULL;
    fiber->task_argc = 0;
    fiber->task_exc_class = NULL;
    fiber->task_exc_message = NULL;
    ZVAL_UNDEF(&fiber->task_retval);
    ZVAL_UNDEF(&fiber->task_closure);

    /* Link into the active list */
    fiber->prev = sched->fibers_tail;
    fiber->next = NULL;
    if (sched->fibers_tail) {
        sched->fibers_tail->next = fiber;
    } else {
        sched->fibers_head = fiber;
    }
    sched->fibers_tail = fiber;
    sched->fiber_count++;

    /* Reconstruct the closure into the fiber's owned task_fci/task_fcc.
     * On the same thread as the call, so MAP_PTR fixups apply correctly. */
    char *exc_class = NULL;
    char *exc_message = NULL;
    if (oxphp_reconstruct_async_closure(
            (zend_op_array *)op_array, (HashTable *)static_vars, (zval *)this_ptr,
            &fiber->task_closure, &fiber->task_fci, &fiber->task_fcc,
            &exc_class, &exc_message) != 0) {
        /* Reconstruction failed — surface as a completed-with-exception
         * fiber so the driver gets a normal error result via poll. The
         * fiber is never entered; its context (if newly created) stays
         * valid and free_list-safe. */
        fiber->task_exc_class = exc_class;
        fiber->task_exc_message = exc_message;
        fiber->completed = true;
        return (int64_t)fiber->fiber_id;
    }

    /* args are borrowed from the Rust driver (freed by it after release) */
    fiber->task_args = (zval *)args;
    fiber->task_argc = argc;

    /* Run to the first suspend or to completion. A recycled fiber is started,
     * never resumed: a resume is what a suspended task gets. */
    oxphp_task_start_fiber(sched, fiber);

    return (int64_t)fiber->fiber_id;
}

bool oxphp_async_sched_io_backoff(int64_t ns) {
    if (!oxphp_task_sched_inited) return false;
    return oxphp_scheduler_io_backoff(&oxphp_task_sched, ns);
}

int oxphp_async_sched_tick(void) {
    if (!oxphp_task_sched_inited) {
        return 0;
    }
    oxphp_fiber_scheduler *sched = &oxphp_task_sched;

    /* Resume fibers whose awaited promise is ready */
    {
        oxphp_request_fiber *fiber = sched->fibers_head;
        while (fiber) {
            oxphp_request_fiber *next = fiber->next;
            if (!fiber->completed && fiber->suspend_reason == OXPHP_SUSPEND_AWAIT) {
                /* Resume when the awaited promise is ready, or when the task
                 * was cancelled — the suspend point unwinds on cancellation
                 * regardless of whether a result has arrived. A ready result or
                 * a cancellation takes precedence over the deadline so a promise
                 * that settles on the same tick still delivers its value. */
                if (fiber->cancel_requested
                    || oxphp_bridge_await_poll(fiber->suspend_data.promise_id)) {
                    oxphp_fiber_clear_suspend(fiber);
                    oxphp_task_resume_fiber(sched, fiber, NULL);
                } else if (fiber->await_deadline_ns != 0) {
                    /* Per-call await timeout: unwind the await once the deadline
                     * elapses so the cooperative fiber path honours the timeout
                     * instead of blocking until the promise settles. */
                    struct timespec ts;
                    clock_gettime(CLOCK_MONOTONIC, &ts);
                    uint64_t now_ns = (uint64_t)ts.tv_sec * 1000000000ULL
                                      + (uint64_t)ts.tv_nsec;
                    if (now_ns >= fiber->await_deadline_ns) {
                        fiber->timed_out = true;
                        oxphp_fiber_clear_suspend(fiber);
                        oxphp_task_resume_fiber(sched, fiber, NULL);
                    }
                }
            }
            fiber = next;
        }
    }

    /* Force-resume sleeping or descriptor-waiting fibers that were cancelled
     * (awaiter gave up) — the suspend point unwinds on cancellation instead of
     * waiting out the timer or the peer. Mirrors the await branch above;
     * without it a task parked in oxphp_sleep()/oxphp_usleep() or in a hooked
     * socket read would run its full duration before unwinding. */
    {
        oxphp_request_fiber *fiber = sched->fibers_head;
        while (fiber) {
            oxphp_request_fiber *next = fiber->next;
            if (!fiber->completed
                && (fiber->suspend_reason == OXPHP_SUSPEND_SLEEP
                    || fiber->suspend_reason == OXPHP_SUSPEND_IO_WAIT)
                && fiber->cancel_requested) {
                oxphp_fiber_clear_suspend(fiber);
                oxphp_task_resume_fiber(sched, fiber, NULL);
            }
            fiber = next;
        }
    }

    /* Resume fibers whose sleep timer expired */
    {
        uint64_t ready_ids[32];
        uint32_t count = oxphp_bridge_timer_poll(ready_ids, 32);
        for (uint32_t i = 0; i < count; i++) {
            oxphp_request_fiber *fiber = sched->fibers_head;
            while (fiber) {
                oxphp_request_fiber *next = fiber->next;
                if (!fiber->completed && fiber->suspend_reason == OXPHP_SUSPEND_SLEEP
                    && fiber->suspend_data.timer_id == ready_ids[i]) {
                    oxphp_fiber_clear_suspend(fiber);
                    oxphp_task_resume_fiber(sched, fiber, NULL);
                    break;
                }
                fiber = next;
            }
        }
    }

    /* Resume fibers whose descriptor became ready (or whose read/write
     * deadline elapsed) — the task-side mirror of the HTTP scheduler's
     * readiness pass, so hooked socket I/O multiplexes async tasks too. */
    {
        oxphp_request_fiber *ready[OXPHP_MAX_FIBERS];
        uint32_t count = oxphp_io_collect_ready(sched, ready, OXPHP_MAX_FIBERS);
        for (uint32_t i = 0; i < count; i++) {
            /* Same per-entry re-check as the HTTP scheduler's readiness pass:
             * the list is read once before the resumes, so each entry is
             * confirmed still parked before it is woken. */
            if (ready[i]->completed
                || ready[i]->suspend_reason != OXPHP_SUSPEND_IO_WAIT) {
                continue;
            }
            oxphp_fiber_clear_suspend(ready[i]);
            oxphp_task_resume_fiber(sched, ready[i], NULL);
        }
    }

    return (int)sched->fiber_count;
}

int64_t oxphp_async_sched_poll_completed(void **out_retval,
                                         const char **out_exc_class,
                                         const char **out_exc_message) {
    if (out_retval != NULL) {
        *out_retval = NULL;
    }
    if (out_exc_class != NULL) {
        *out_exc_class = NULL;
    }
    if (out_exc_message != NULL) {
        *out_exc_message = NULL;
    }
    if (!oxphp_task_sched_inited) {
        return -1;
    }
    oxphp_fiber_scheduler *sched = &oxphp_task_sched;

    for (oxphp_request_fiber *fiber = sched->fibers_head; fiber; fiber = fiber->next) {
        if (fiber->completed) {
            if (out_retval != NULL) {
                *out_retval = &fiber->task_retval;
            }
            if (out_exc_class != NULL) {
                *out_exc_class = fiber->task_exc_class;
            }
            if (out_exc_message != NULL) {
                *out_exc_message = fiber->task_exc_message;
            }
            return (int64_t)fiber->fiber_id;
        }
    }
    return -1;
}

/* Tear down a finished task fiber and hand it to the reuse pool. The caller has
 * established fiber->completed; the driver has already drained its retval. */
static void oxphp_task_recycle_fiber(oxphp_fiber_scheduler *sched,
                                     oxphp_request_fiber *fiber) {
    oxphp_fiber_free_task_payload(fiber);
    fiber->task_args = NULL;
    fiber->task_argc = 0;

    /* Unlink from the active list */
    if (fiber->prev) {
        fiber->prev->next = fiber->next;
    } else {
        sched->fibers_head = fiber->next;
    }
    if (fiber->next) {
        fiber->next->prev = fiber->prev;
    } else {
        sched->fibers_tail = fiber->prev;
    }
    sched->fiber_count--;

    /* Recycle: the looping coroutine keeps the C stack alive for reuse.
     * zend_fiber_destroy_context only runs in oxphp_scheduler_destroy. Clear the
     * suspension first, so no fiber sits on the free list still describing a
     * descriptor wait. */
    oxphp_fiber_clear_suspend(fiber);
    fiber->next = sched->free_list;
    sched->free_list = fiber;
}

void oxphp_async_sched_release(int64_t fiber_id) {
    if (!oxphp_task_sched_inited) {
        return;
    }
    oxphp_fiber_scheduler *sched = &oxphp_task_sched;

    oxphp_request_fiber *fiber = sched->fibers_head;
    while (fiber && (int64_t)fiber->fiber_id != fiber_id) {
        fiber = fiber->next;
    }
    if (!fiber) {
        return;
    }

    /* Guard against a future caller, not against a state the current one can
     * reach: the driver releases only what poll_completed has just handed back
     * (src/executor/async_pool.rs), so the coroutine is parked at the bottom of
     * its loop by construction. It stays a runtime check because the contract
     * crosses an FFI boundary, and because releasing a still-suspended fiber
     * would later resume it into a foreign closure and free the payload its own
     * task is running on. ZEND_ASSERT cannot stand in: in a release build it is
     * ZEND_ASSUME, which would license the compiler to delete this branch. */
    if (!fiber->completed) {
        char msg[160];
        snprintf(msg, sizeof(msg),
                 "oxphp: async task fiber %llu is still suspended — not recycling it",
                 (unsigned long long)fiber->fiber_id);
        php_log_err(msg);
        return;
    }

    oxphp_task_recycle_fiber(sched, fiber);
}

int oxphp_async_sched_cancel(int64_t fiber_id) {
    if (!oxphp_task_sched_inited) {
        return 0;
    }
    oxphp_fiber_scheduler *sched = &oxphp_task_sched;

    for (oxphp_request_fiber *fiber = sched->fibers_head; fiber; fiber = fiber->next) {
        if ((int64_t)fiber->fiber_id == fiber_id) {
            fiber->cancel_requested = true;
            return 1;
        }
    }
    return 0;
}

void oxphp_async_sched_shutdown(void) {
    /* Tear down this thread's lazily-created task scheduler. The async worker
     * reaches this once via its single php_request_shutdown at thread exit
     * (through the extension RSHUTDOWN), while the executor heap is still live
     * for the zval dtors. Frees every active and free-list fiber's mmap'd C
     * stack and task payload. A no-op on threads that never spawned a task
     * (HTTP workers, bare CLI), and idempotent — `inited` is cleared so a
     * second shutdown finds nothing. */
    if (!oxphp_task_sched_inited) {
        return;
    }
    oxphp_scheduler_destroy(&oxphp_task_sched);
    oxphp_task_sched_inited = false;
}
