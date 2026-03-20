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
#include "php_output.h"

/* ─── TLS: current fiber pointer ───────────────────────── */

__thread oxphp_request_fiber *oxphp_current_fiber = NULL;

/* ─── Forward declarations ─────────────────────────────── */

static void request_fiber_coroutine(zend_fiber_transfer *transfer);

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
        if (fiber->context.status != ZEND_FIBER_STATUS_DEAD) {
            zend_fiber_destroy_context(&fiber->context);
        }
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

/* Coroutine entry point for each request fiber.
 *
 * IMPORTANT: On entry, transfer->context is the CALLER's (scheduler's) context,
 * NOT our context. Our fiber pointer is stored in our context's `kind` field,
 * accessible via EG(current_fiber_context)->kind after the switch. */
static void request_fiber_coroutine(zend_fiber_transfer *transfer) {
    /* Retrieve fiber pointer via kind — set during zend_fiber_init_context() */
    oxphp_request_fiber *fiber = (oxphp_request_fiber *)EG(current_fiber_context)->kind;
    fiber->scheduler = transfer->context; /* remember caller (scheduler) context */

    /* Set TLS so oxphp_async_await/oxphp_sleep can detect fiber mode */
    oxphp_current_fiber = fiber;

    /* Call the PHP handler with zend_try protection */
    zval retval;
    zend_execute_data *saved_execute_data = EG(current_execute_data);
    ZVAL_UNDEF(&retval);

    zend_try {
        fiber->fci->retval = &retval;
        fiber->fci->param_count = 0;
        fiber->fci->params = NULL;
        if (zend_call_function(fiber->fci, fiber->fcc) == SUCCESS) {
            zval_ptr_dtor(&retval);
        }
        /* PHP 8.4: exit/die throws UnwindExit instead of bailout */
        if (EG(exception)) {
            if (!zend_is_unwind_exit(EG(exception)) && !zend_is_graceful_exit(EG(exception))) {
                fiber->handler_failed = true;
            }
            OBJ_RELEASE(EG(exception));
            EG(exception) = NULL;
        }
    } zend_catch {
        /* Actual zend_bailout: fatal error, timeout, cancellation */
        fiber->handler_failed = true;
        EG(current_execute_data) = saved_execute_data;
        if (EG(exception)) {
            OBJ_RELEASE(EG(exception));
            EG(exception) = NULL;
        }
        CG(unclean_shutdown) = 0;
    } zend_end_try();

    /* Run shutdown functions for this request */
    php_call_shutdown_functions();
    php_free_shutdown_functions();

    oxphp_current_fiber = NULL;

    /* Return to scheduler. Do NOT set fiber->context.status manually —
     * zend_fiber_switch_context and zend_fiber_destroy_context manage it.
     * The scheduler detects completion by checking status after the switch. */
    zend_fiber_transfer ret = { .context = fiber->scheduler, .flags = 0 };
    ZVAL_NULL(&ret.value);
    zend_fiber_switch_context(&ret);
    /* Never reached — scheduler destroys our context */
}

/* ─── Fiber Creation ───────────────────────────────────── */

oxphp_request_fiber *oxphp_scheduler_create_fiber(
    oxphp_fiber_scheduler *sched,
    zend_fcall_info *fci,
    zend_fcall_info_cache *fcc
) {
    /* Reuse from free list or allocate new */
    oxphp_request_fiber *fiber;
    if (sched->free_list) {
        fiber = sched->free_list;
        sched->free_list = fiber->next;
    } else {
        fiber = ecalloc(1, sizeof(oxphp_request_fiber));
    }

    memset(fiber, 0, sizeof(*fiber));
    fiber->fiber_id = sched->next_fiber_id++;
    fiber->fci = fci;
    fiber->fcc = fcc;
    fiber->suspend_reason = OXPHP_SUSPEND_NONE;

    /* Initialize fiber context.
     * Pass `fiber` as `kind` — the coroutine reads it back via
     * EG(current_fiber_context)->kind to get its oxphp_request_fiber*. */
    if (zend_fiber_init_context(
            &fiber->context,
            (void *)fiber, /* kind = fiber pointer for coroutine access */
            request_fiber_coroutine,
            EG(fiber_stack_size)) != SUCCESS) {
        efree(fiber);
        return NULL;
    }

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

    zend_fiber_transfer transfer = { .context = &fiber->context, .flags = 0 };
    ZVAL_NULL(&transfer.value);

    zend_fiber_switch_context(&transfer);

    /* Back in scheduler — fiber either suspended or completed */
    sched->current = NULL;

    if (fiber->context.status == ZEND_FIBER_STATUS_DEAD) {
        /* Handler completed without suspending — fast path */
        return;
    }

    /* Fiber suspended — save its PHP state */
    oxphp_fiber_save_php_state(fiber);
}

void oxphp_scheduler_resume_fiber(oxphp_fiber_scheduler *sched, oxphp_request_fiber *fiber, zval *value) {
    sched->current = fiber;

    /* Restore fiber's PHP state */
    oxphp_fiber_restore_php_state(fiber);

    oxphp_current_fiber = fiber;

    zend_fiber_transfer transfer = { .context = &fiber->context, .flags = 0 };
    if (value) {
        ZVAL_COPY_VALUE(&transfer.value, value);
    } else {
        ZVAL_NULL(&transfer.value);
    }

    zend_fiber_switch_context(&transfer);

    oxphp_current_fiber = NULL;
    sched->current = NULL;

    if (fiber->context.status != ZEND_FIBER_STATUS_DEAD) {
        /* Suspended again — save state */
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
    sched->total_requests_done++;

    /* Send the HTTP response via Rust (same as worker_send_callback) */
    oxphp_bridge_worker_send_response();

    /* Drop the fiber's Rust TLS slot (RESPONSE, EARLY_TX, REQUEST_DATA).
     * Must happen AFTER send_response since the response reads from RESPONSE TLS. */
    oxphp_bridge_fiber_drop_ctx(fiber->fiber_id);

    /* Destroy fiber context */
    if (fiber->context.status == ZEND_FIBER_STATUS_DEAD) {
        zend_fiber_destroy_context(&fiber->context);
    }

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

        /* Create and start fiber */
        oxphp_request_fiber *fiber = oxphp_scheduler_create_fiber(
            sched, sched->shared_fci, sched->shared_fcc);
        if (!fiber) break;

        oxphp_scheduler_start_fiber(sched, fiber);

        /* Check if it completed immediately (no suspend) */
        if (fiber->context.status == ZEND_FIBER_STATUS_DEAD) {
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

                    if (fiber->context.status == ZEND_FIBER_STATUS_DEAD) {
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
                    if (fiber->context.status == ZEND_FIBER_STATUS_DEAD) {
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
