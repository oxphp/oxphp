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

/* ─── TLS: current fiber pointer ───────────────────────── */

__thread oxphp_request_fiber *oxphp_current_fiber = NULL;

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

void oxphp_scheduler_destroy(oxphp_fiber_scheduler *sched) {
    /* Free any remaining active fibers */
    oxphp_request_fiber *fiber = sched->fibers_head;
    while (fiber) {
        oxphp_request_fiber *next = fiber->next;
        zend_fiber_destroy_context(&fiber->context);
        oxphp_bridge_fiber_drop_ctx(fiber->fiber_id);
        efree(fiber);
        fiber = next;
    }
    /* Free the free list */
    fiber = sched->free_list;
    while (fiber) {
        oxphp_request_fiber *next = fiber->next;
        efree(fiber);
        fiber = next;
    }
    sched->fibers_head = NULL;
    sched->fibers_tail = NULL;
    sched->free_list = NULL;
    sched->fiber_count = 0;
}

/* ─── Coroutine Entry Point ────────────────────────────── */

/* Looping coroutine: the fiber's C stack is allocated ONCE and reused for all
 * requests assigned to this fiber. After each request completes, the coroutine
 * suspends back to the scheduler (marking completed=true). The scheduler can
 * then resume it for the next request without mmap/munmap overhead.
 *
 * If the handler suspends mid-request (oxphp_sleep/oxphp_async_await), the
 * scheduler creates additional fibers for concurrent requests. */
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
    fiber->suspend_reason = OXPHP_SUSPEND_NONE;
    fiber->handler_failed = false;
    fiber->completed = false;
    fiber->consecutive_errors = 0;

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
    /* Track per-fiber handler failure in scheduler-level counter */
    if (fiber->handler_failed) {
        sched->consecutive_errors++;
    } else {
        sched->consecutive_errors = 0;
    }
    /* Increment moved to request-start path (oxphp_scheduler_create_fiber
     * for the fast path; oxphp_scheduler_tick's new-request branch for
     * the event-loop path) so PHP-side requestCount() observes the
     * current request's index, not the previous count. */

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

        /* Create or reuse fiber */
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

    /* 2. Check completed async awaits */
    {
        oxphp_request_fiber *fiber = sched->fibers_head;
        while (fiber) {
            oxphp_request_fiber *next = fiber->next; /* save — finalize may unlink */

            if (fiber->suspend_reason == OXPHP_SUSPEND_AWAIT) {
                if (oxphp_bridge_await_poll(fiber->suspend_data.promise_id)) {
                    fiber->suspend_reason = OXPHP_SUSPEND_NONE;
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
                    fiber->suspend_reason = OXPHP_SUSPEND_NONE;
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
    fiber->suspend_reason = OXPHP_SUSPEND_NONE;
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
                    fiber->suspend_reason = OXPHP_SUSPEND_NONE;
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
                        fiber->suspend_reason = OXPHP_SUSPEND_NONE;
                        oxphp_task_resume_fiber(sched, fiber, NULL);
                    }
                }
            }
            fiber = next;
        }
    }

    /* Force-resume sleeping fibers that were cancelled (awaiter gave up) —
     * the suspend point unwinds on cancellation instead of waiting out the
     * timer. Mirrors the await branch above; without it a task parked in
     * oxphp_sleep()/oxphp_usleep() would sleep its full duration before
     * unwinding. */
    {
        oxphp_request_fiber *fiber = sched->fibers_head;
        while (fiber) {
            oxphp_request_fiber *next = fiber->next;
            if (!fiber->completed && fiber->suspend_reason == OXPHP_SUSPEND_SLEEP
                && fiber->cancel_requested) {
                fiber->suspend_reason = OXPHP_SUSPEND_NONE;
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
                    fiber->suspend_reason = OXPHP_SUSPEND_NONE;
                    oxphp_task_resume_fiber(sched, fiber, NULL);
                    break;
                }
                fiber = next;
            }
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
