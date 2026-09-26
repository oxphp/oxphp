<?php

declare(strict_types=1);

// A request that ends holding a php://input handle with a userland filter on it,
// where the filter's onclose() fatals.
//
// The end of a request closes the php://input handles still open on its body,
// and closing one tears down its filter chain: a filter a script registered is a
// PHP object, and disposing of it calls the object's onclose(). So arbitrary PHP
// runs from inside the finalization of a request, on a worker that goes on
// serving others afterwards — and a fatal raised there is a longjmp out of that
// finalization unless something catches it. Caught, it still leaves the engine
// with the flags every bailout raises, the collector's protection among them —
// and the engine lowers that one only when it activates, which a worker does
// once, so on a worker nothing else lowers it but the recovery after a bailout.
//
// The handle is parked in the worker-scope store so that the script's own end
// does not release it: the end-of-request close has to be the thing that reaches
// it, or this request exercises an ordinary fclose() instead.
//
// No assertions and no test JSON: the suite line checks the status, and the
// request after this one (test_worker_serves_after_input_filter_close) says
// whether the worker is still there and whether the recovery ran to its end.

// A worker never shuts a request down, so the error handler the TestCase of an
// earlier request installed is still standing on this thread, and it throws on
// every error it is handed. trigger_error() with E_USER_ERROR raises an
// E_DEPRECATED first, as of PHP 8.4; the handler throws on that, and
// trigger_error() returns with the exception without ever raising the
// E_USER_ERROR. An exception is not the fatal this case is about.
set_error_handler(null);

if (!class_exists('OxPHPInputFatalOnCloseFilter', false)) {
    class OxPHPInputFatalOnCloseFilter extends php_user_filter
    {
        public function onClose(): void
        {
            // Recorded before the fatal, so the request after this one can tell
            // a close that reached the handle from one that never got to it.
            @file_put_contents('/tmp/oxphp-input-filter-close-fatal', (string) time());

            trigger_error(
                'fatal raised from a filter on php://input while its request was being finalized',
                E_USER_ERROR
            );
        }

        /** @param resource $in @param resource $out */
        public function filter($in, $out, &$consumed, bool $closing): int
        {
            return PSFS_PASS_ON;
        }
    }
}

// The filter map is per thread and outlives the request on a worker, so once a
// request on this worker has registered the name it stays registered. Registering
// it again would return false and do nothing else, so the check only skips a call
// known to fail.
if (!in_array('oxphp-input-fatal-on-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-input-fatal-on-close', OxPHPInputFatalOnCloseFilter::class);
}

if (is_file('/tmp/oxphp-input-filter-close-fatal')) {
    unlink('/tmp/oxphp-input-filter-close-fatal');
}

$sharedState['php_input_filter_close'] = fopen('php://input', 'r');
stream_filter_append($sharedState['php_input_filter_close'], 'oxphp-input-fatal-on-close', STREAM_FILTER_READ);

echo "ARMED\n";
