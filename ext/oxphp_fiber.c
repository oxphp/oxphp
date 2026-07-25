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
 * - VM state saved/restored automatically by zend_fiber_switch_context
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
#include <poll.h>   /* poll() for descriptor readiness of IO_WAIT-suspended fibers */
#include <errno.h>  /* EINTR from the readiness poll */
#include <stdatomic.h> /* one-shot flag for the readiness-poll failure log */

/* ─── TLS: current fiber pointer ───────────────────────── */

__thread oxphp_request_fiber *oxphp_current_fiber = NULL;

uint64_t oxphp_fiber_current_id(void) {
    return oxphp_current_fiber ? oxphp_current_fiber->fiber_id : 0;
}

/* ─── Forward declarations ─────────────────────────────── */

static void request_fiber_coroutine(zend_fiber_transfer *transfer);

/* ─── VM State Save/Restore ────────────────────────────── */

/* The low-level fiber API (zend_fiber_switch_context) does NOT save/restore
 * Zend VM state. The high-level API does this via zend_fiber_save_vm_state
 * which is static (not exported). We replicate it here.
 *
 * Without this, concurrent fibers corrupt each other's:
 * - vm_stack (PHP temporary allocations)
 * - execute_data (call frame chain)
 * - bailout (setjmp buffer for zend_try — longjmp to wrong stack = SIGSEGV) */

static inline void oxphp_save_vm_state(oxphp_fiber_vm_state *state) {
    state->vm_stack = EG(vm_stack);
    state->vm_stack_top = EG(vm_stack_top);
    state->vm_stack_end = EG(vm_stack_end);
    state->vm_stack_page_size = EG(vm_stack_page_size);
    state->current_execute_data = EG(current_execute_data);
    state->error_reporting = EG(error_reporting);
    state->jit_trace_num = EG(jit_trace_num);
    state->bailout = EG(bailout);
    state->active_fiber = EG(active_fiber);
}

static inline void oxphp_restore_vm_state(oxphp_fiber_vm_state *state) {
    EG(vm_stack) = state->vm_stack;
    EG(vm_stack_top) = state->vm_stack_top;
    EG(vm_stack_end) = state->vm_stack_end;
    EG(vm_stack_page_size) = state->vm_stack_page_size;
    EG(current_execute_data) = state->current_execute_data;
    EG(error_reporting) = state->error_reporting;
    EG(jit_trace_num) = state->jit_trace_num;
    EG(bailout) = state->bailout;
    EG(active_fiber) = state->active_fiber;
}

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

/* ─── Scheduler Init / Destroy ─────────────────────────── */

void oxphp_scheduler_init(oxphp_fiber_scheduler *sched) {
    memset(sched, 0, sizeof(*sched));
    sched->next_fiber_id = 1;
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
}

void oxphp_scheduler_destroy(oxphp_fiber_scheduler *sched) {
    /* Free any remaining active fibers */
    oxphp_request_fiber *fiber = sched->fibers_head;
    while (fiber) {
        oxphp_request_fiber *next = fiber->next;
        zend_fiber_destroy_context(&fiber->context);
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
        zend_fiber_destroy_context(&fiber->context);
        oxphp_fiber_free_task_payload(fiber);
        efree(fiber);
        fiber = next;
    }
    sched->fibers_head = NULL;
    sched->fibers_tail = NULL;
    sched->free_list = NULL;
    sched->fiber_count = 0;

    free(sched->io_fds);
    free(sched->io_owners);
    sched->io_fds = NULL;
    sched->io_owners = NULL;
    sched->io_cap = 0;
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

static void request_fiber_coroutine(zend_fiber_transfer *transfer) {
    /* Retrieve fiber pointer via kind — set during zend_fiber_init_context() */
    oxphp_request_fiber *fiber = (oxphp_request_fiber *)EG(current_fiber_context)->kind;
    fiber->scheduler = transfer->context;

    /* Set stack overflow detection limits ONCE (C stack is reused) */
    int stack_anchor;
    oxphp_fiber_set_stack_limits_from_sp(&stack_anchor, EG(fiber_stack_size));
    fiber->saved_stack_base = EG(stack_base);
    fiber->saved_stack_limit = EG(stack_limit);

    /* ── Request processing loop ────────────────────────── */
    for (;;) {
        oxphp_current_fiber = fiber;

        /* Allocate fresh VM stack per request (cheap emalloc, not mmap) */
        EG(vm_stack) = zend_vm_stack_new_page(ZEND_FIBER_VM_STACK_SIZE, NULL);
        EG(vm_stack_top) = EG(vm_stack)->top;
        EG(vm_stack_end) = EG(vm_stack)->end;
        EG(vm_stack_page_size) = ZEND_FIBER_VM_STACK_SIZE;
        EG(current_execute_data) = NULL;
        /* Start uncoupled from any in-progress trace recording (upstream
         * zend_fiber_execute does the same on coroutine entry). */
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

        oxphp_current_fiber = NULL;

        /* Destroy this request's VM stack (emalloc'd, not mmap'd — cheap) */
        zend_vm_stack_destroy();

        /* Mark completed and suspend back to scheduler.
         * Scheduler will resume us for the next request (looping coroutine). */
        fiber->completed = true;

        zend_fiber_transfer ret = { .context = fiber->scheduler, .flags = 0 };
        ZVAL_NULL(&ret.value);
        zend_fiber_switch_context(&ret);

        /* ── RESUMED for next request ────────────────────────
         * Scheduler has called prepare_request + init_request_state
         * and restored our VM state before resuming. Reset per-request state. */
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
        fiber->suspend_data.io.fds = NULL;
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
    fiber->fci = fci;
    fiber->fcc = fcc;
    oxphp_fiber_clear_suspend(fiber);
    fiber->handler_failed = false;
    fiber->completed = false;
    fiber->consecutive_errors = 0;
    fiber->drain_kill = false;
    /* A free-list fiber's saved php_state still holds the previous request's
     * final connection_status and bridge ctx flags (plain ints and bools —
     * unlike the zval fields they are not consumed by restore). Reset them, or
     * the first resume for THIS request would re-install the previous
     * request's ABORTED status over the fresh PHP_CONNECTION_NORMAL that
     * oxphp_fiber_init_request_state() just set, and re-install its streaming
     * / finished flags over the ones oxphp_bridge_reset_request_ctx() just
     * cleared — a fresh request would start out believing its response had
     * already been sent. */
    fiber->php_state.connection_status = PHP_CONNECTION_NORMAL;
    fiber->php_state.bridge_stream_mode = false;
    fiber->php_state.bridge_headers_sent = false;
    fiber->php_state.bridge_finished = false;
    /* Capture this request's cancel cell here, in the one place both request
     * paths share: the Rust prep (setup_request_tls) installed it into the
     * bridge ctx immediately before this call — via worker_wait on the fast
     * path, via prepare_request on the event-loop path. Doing it at creation
     * also clears a free-list fiber's stale pointer from its previous request
     * (dangling once that request's CancellationState is dropped). Re-installed
     * into the bridge ctx on every resume — see oxphp_fiber_restore_php_state. */
    fiber->request_cancel_ptr = oxphp_bridge_get_cancel_ptr();

    if (!reused) {
        /* First-time init: allocate C stack via mmap */
        if (zend_fiber_init_context(
                &fiber->context,
                (void *)fiber,
                request_fiber_coroutine,
                EG(fiber_stack_size)) != SUCCESS) {
            efree(fiber);
            return NULL;
        }
    }
    /* Reused fibers: coroutine is looping inside zend_fiber_switch_context,
     * waiting to be resumed. No re-init needed. */

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

    /* Step 5: Save VM state (vm_stack, execute_data, bailout) */
    oxphp_save_vm_state(&fiber->php_state.vm_state);
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
    for (int i = 0; i < 6; i++) {
        zval_ptr_dtor_nogc(&PG(http_globals)[i]);
        ZVAL_UNDEF(&PG(http_globals)[i]);
    }
    zend_activate_auto_globals();

    /* Force $_SERVER population */
    zend_is_auto_global(ZSTR_KNOWN(ZEND_STR_AUTOGLOBAL_SERVER));

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

void oxphp_scheduler_start_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber) {
    sched->current = fiber;

    /* Save scheduler's VM state + stack limits */
    oxphp_fiber_vm_state saved_vm;
    oxphp_save_vm_state(&saved_vm);
    void *saved_stack_base = EG(stack_base);
    void *saved_stack_limit = EG(stack_limit);

    zend_fiber_transfer transfer = { .context = &fiber->context, .flags = 0 };
    ZVAL_NULL(&transfer.value);

    zend_fiber_switch_context(&transfer);

    /* Back in scheduler — restore VM state + stack limits */
    oxphp_restore_vm_state(&saved_vm);
    EG(stack_base) = saved_stack_base;
    EG(stack_limit) = saved_stack_limit;
    sched->current = NULL;

    if (fiber->completed) {
        /* Handler completed without suspending */
        return;
    }

    /* Fiber suspended — save its PHP + VM state */
    oxphp_fiber_save_php_state(fiber);
}

void oxphp_scheduler_resume_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber, zval *value) {
    sched->current = fiber;

    /* Restore fiber's PHP state (superglobals, SAPI headers, Rust TLS) */
    oxphp_fiber_restore_php_state(fiber);

    oxphp_current_fiber = fiber;

    /* Save scheduler's VM state + stack limits */
    oxphp_fiber_vm_state saved_vm;
    oxphp_save_vm_state(&saved_vm);
    void *saved_stack_base = EG(stack_base);
    void *saved_stack_limit = EG(stack_limit);

    /* Restore fiber's VM state + stack limits */
    oxphp_restore_vm_state(&fiber->php_state.vm_state);
    EG(stack_base) = fiber->saved_stack_base;
    EG(stack_limit) = fiber->saved_stack_limit;

    zend_fiber_transfer transfer = { .context = &fiber->context, .flags = 0 };
    if (value) {
        ZVAL_COPY_VALUE(&transfer.value, value);
    } else {
        ZVAL_NULL(&transfer.value);
    }

    zend_fiber_switch_context(&transfer);

    /* Back in scheduler — restore VM state + stack limits */
    oxphp_restore_vm_state(&saved_vm);
    EG(stack_base) = saved_stack_base;
    EG(stack_limit) = saved_stack_limit;
    oxphp_current_fiber = NULL;
    sched->current = NULL;

    if (!fiber->completed) {
        /* Suspended again — save VM + PHP state */
        oxphp_fiber_save_php_state(fiber);
    }
}

void oxphp_scheduler_finalize_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber) {
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

    /* Return to free list */
    fiber->next = sched->free_list;
    sched->free_list = fiber;
}

/* ─── Descriptor readiness for IO_WAIT-suspended fibers ──
 *
 * Collects every fiber parked on a descriptor, polls them all in one
 * non-blocking poll(), and hands back those that may run again — either
 * because the descriptor is ready or because their deadline elapsed. The
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
 * a suspended fiber is only resumable from the thread that owns it. POLLHUP
 * and POLLERR count as ready — the delegated caller has to observe EOF and
 * socket errors itself, exactly as it would without a hook. */

/* Grow the scheduler's aggregated poll set to hold at least `need` entries.
 * Returns false when the allocation fails, which the callers treat as "nothing
 * to wait on" — degrading to the blocking native path rather than crashing. */
static bool oxphp_io_reserve(oxphp_fiber_scheduler *sched, uint32_t need) {
    if (need <= sched->io_cap) return true;

    uint32_t cap = sched->io_cap ? sched->io_cap : 16;
    while (cap < need) cap *= 2;

    struct pollfd *fds = realloc(sched->io_fds, (size_t)cap * sizeof(*fds));
    if (fds == NULL) return false;
    sched->io_fds = fds;

    struct oxphp_io_owner *owners =
        realloc(sched->io_owners, (size_t)cap * sizeof(*owners));
    if (owners == NULL) return false;
    sched->io_owners = owners;

    sched->io_cap = cap;
    return true;
}

static uint32_t oxphp_io_build_pollset(oxphp_fiber_scheduler *sched) {
    uint32_t need = 0;
    for (oxphp_request_fiber *fiber = sched->fibers_head; fiber; fiber = fiber->next) {
        if (fiber->completed || fiber->suspend_reason != OXPHP_SUSPEND_IO_WAIT) {
            continue;
        }
        need += fiber->suspend_data.io.nfds;
    }
    if (need == 0 || !oxphp_io_reserve(sched, need)) return 0;

    uint32_t n = 0;
    for (oxphp_request_fiber *fiber = sched->fibers_head; fiber; fiber = fiber->next) {
        if (fiber->completed || fiber->suspend_reason != OXPHP_SUSPEND_IO_WAIT) {
            continue;
        }
        for (uint32_t i = 0; i < fiber->suspend_data.io.nfds; i++) {
            sched->io_fds[n] = fiber->suspend_data.io.fds[i];
            sched->io_fds[n].revents = 0;
            sched->io_owners[n].fiber = fiber;
            sched->io_owners[n].idx = i;
            n++;
        }
    }
    return n;
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
    if (ns < 0) return false;

    uint32_t nfds = oxphp_io_build_pollset(sched);
    if (nfds == 0) return false;

    /* A signal cuts the pause short; unlike php_sock_stream_wait_for_data this
     * does not retry on EINTR, because there is nothing to preserve — the
     * caller loops, and an early return costs it one extra turn round that
     * loop, not a spin: a profiler sampling at 100 Hz skips one pause per
     * 10 ms. Readiness and deadlines are decided by oxphp_io_collect_ready(),
     * never here.
     *
     * Any other failure is reported as "did not wait" so the caller sleeps its
     * own interval instead. Claiming to have waited when the call failed would
     * turn a persistent error into a busy loop at full CPU with nothing in the
     * log to explain it. */
    struct timespec timeout = {
        .tv_sec = (time_t)(ns / 1000000000),
        .tv_nsec = (long)(ns % 1000000000),
    };
    if (ppoll(sched->io_fds, nfds, &timeout, NULL) < 0 && errno != EINTR) {
        return false;
    }
    return true;
}

static uint32_t oxphp_io_collect_ready(oxphp_fiber_scheduler *sched,
                                       oxphp_request_fiber **out, uint32_t max) {
    uint32_t nfds = oxphp_io_build_pollset(sched);
    if (nfds == 0) return 0;

    if (poll(sched->io_fds, nfds, 0) < 0 && errno != EINTR) {
        /* poll() reports a closed or invalid descriptor per entry, via POLLNVAL
         * in its revents, so this is the whole-call failure (EINVAL, ENOMEM):
         * nothing was examined and every waiter would otherwise stay parked
         * forever. Release them all rather than strand them.
         *
         * Be clear about what that costs: each released fiber goes on to its
         * delegate, which polls its own descriptor again, usually succeeds, and
         * blocks the worker thread for the socket's full timeout — the very
         * thing the hook exists to avoid. The hook degrades to native
         * behaviour, which is survivable, but it is silent otherwise, so say it
         * once. Once for the process, not once per worker: every thread that
         * hooks sockets would otherwise repeat the same line, and the failure
         * is a property of the platform, not of the thread that noticed it.
         * The exchange is what makes "once" true when two workers notice
         * together. */
        static atomic_bool reported = false;
        if (!atomic_exchange(&reported, true)) {
            php_log_err("oxphp: polling parked socket descriptors failed; hooked reads "
                        "fall back to blocking the worker thread until it recovers");
        }
        for (uint32_t i = 0; i < nfds; i++) {
            sched->io_fds[i].revents = POLLERR;
        }
    }

    /* Scatter readiness back into each fiber's own array: that is where the
     * suspended code reads it, and a fiber waiting on several descriptors needs
     * to know which of them fired, not merely that one did. */
    for (uint32_t i = 0; i < nfds; i++) {
        struct oxphp_io_owner *owner = &sched->io_owners[i];
        owner->fiber->suspend_data.io.fds[owner->idx].revents = sched->io_fds[i].revents;
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
            if (fiber->suspend_data.io.fds[i].revents != 0) { any = true; break; }
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

        if (fiber->started) {
            oxphp_scheduler_resume_fiber(sched, fiber, NULL);
        } else {
            fiber->started = true;
            oxphp_scheduler_start_fiber(sched, fiber);
        }

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
     * of blocking the worker thread inside poll(). */
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
 * Reuses the HTTP fiber struct (oxphp_request_fiber + its task_* fields)
 * and the VM-state save/restore helpers. Unlike HTTP request fibers,
 * task fibers carry no per-request superglobal / SAPI-header / Rust-TLS
 * state: only the Zend VM state (vm_stack / execute_data / bailout) needs
 * per-fiber save/restore so concurrent suspended tasks do not corrupt
 * each other. Superglobals and output buffers stay shared on the worker
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

        oxphp_current_fiber = NULL;

        /* Destroy this task's VM stack (emalloc'd — cheap) */
        zend_vm_stack_destroy();

        /* Mark completed and suspend back to scheduler. The fiber stays in
         * the active list until the driver drains it (poll) and releases it. */
        fiber->completed = true;

        zend_fiber_transfer ret = { .context = fiber->scheduler, .flags = 0 };
        ZVAL_NULL(&ret.value);
        zend_fiber_switch_context(&ret);

        /* ── Resumed to run the next task (recycled fiber) ──
         * spawn has reconstructed a fresh closure into task_fci/task_fcc,
         * set task_args/argc, and cleared `completed` before resuming. */
    }
}

/* Switch into a task fiber for the first time (fresh context, coroutine
 * begins at its entry). VM-state-only mirror of oxphp_scheduler_start_fiber. */
static void oxphp_task_start_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber) {
    sched->current = fiber;

    oxphp_fiber_vm_state saved_vm;
    oxphp_save_vm_state(&saved_vm);
    void *saved_stack_base = EG(stack_base);
    void *saved_stack_limit = EG(stack_limit);

    zend_fiber_transfer transfer = { .context = &fiber->context, .flags = 0 };
    ZVAL_NULL(&transfer.value);
    zend_fiber_switch_context(&transfer);

    oxphp_restore_vm_state(&saved_vm);
    EG(stack_base) = saved_stack_base;
    EG(stack_limit) = saved_stack_limit;
    sched->current = NULL;

    if (fiber->completed) {
        return;
    }
    /* Suspended — save its VM state for later resume (no HTTP request state) */
    oxphp_save_vm_state(&fiber->php_state.vm_state);
}

/* Resume a suspended (or recycled) task fiber. VM-state-only mirror of
 * oxphp_scheduler_resume_fiber. */
static void oxphp_task_resume_fiber(oxphp_fiber_scheduler *sched,
                                    oxphp_request_fiber *fiber, zval *value) {
    sched->current = fiber;
    oxphp_current_fiber = fiber;

    oxphp_fiber_vm_state saved_vm;
    oxphp_save_vm_state(&saved_vm);
    void *saved_stack_base = EG(stack_base);
    void *saved_stack_limit = EG(stack_limit);

    oxphp_restore_vm_state(&fiber->php_state.vm_state);
    EG(stack_base) = fiber->saved_stack_base;
    EG(stack_limit) = fiber->saved_stack_limit;

    zend_fiber_transfer transfer = { .context = &fiber->context, .flags = 0 };
    if (value) {
        ZVAL_COPY_VALUE(&transfer.value, value);
    } else {
        ZVAL_NULL(&transfer.value);
    }
    zend_fiber_switch_context(&transfer);

    oxphp_restore_vm_state(&saved_vm);
    EG(stack_base) = saved_stack_base;
    EG(stack_limit) = saved_stack_limit;
    oxphp_current_fiber = NULL;
    sched->current = NULL;

    if (!fiber->completed) {
        oxphp_save_vm_state(&fiber->php_state.vm_state);
    }
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
        fiber->started = false;
    }

    fiber->fiber_id = sched->next_fiber_id++;
    fiber->task_mode = true;
    fiber->cancel_requested = false;
    fiber->timed_out = false;
    fiber->await_deadline_ns = 0;
    fiber->cancel_cell = (_Atomic(uint8_t) *)cancel_cell;
    fiber->request_cancel_ptr = NULL; /* task fibers carry no HTTP request cell */
    fiber->drain_kill = false;
    fiber->php_state.bridge_stream_mode = false;
    fiber->php_state.bridge_headers_sent = false;
    fiber->php_state.bridge_finished = false;
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

    /* Run to the first suspend or to completion */
    if (fiber->started) {
        oxphp_task_resume_fiber(sched, fiber, NULL);
    } else {
        fiber->started = true;
        oxphp_task_start_fiber(sched, fiber);
    }

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

    /* Tear down task-owned state (the driver has already drained retval) */
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
     * zend_fiber_destroy_context only runs in oxphp_scheduler_destroy. */
    fiber->next = sched->free_list;
    sched->free_list = fiber;
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
