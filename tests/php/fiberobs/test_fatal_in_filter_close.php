<?php

declare(strict_types=1);

// A fatal raised by userland code that the recovery from an earlier fatal
// reaches on its way through.
//
// A bailout abandons the frames it was inside, and the worker has to give back
// what they were holding before it can serve anything else. One of the things a
// frame can hold is a stream handle, and giving one back is not a leaf
// operation: closing a stream disposes its filter chains, and disposing a
// userland filter calls the filter object's onclose(). So the release walk
// reaches ordinary application code, and a fatal raised in there is raised
// inside the recovery itself — past the point where the bailout target has
// already been handed back to the caller's caller. Left unguarded it jumps over
// the rest of the recovery and out of the loop that serves requests, taking
// every request multiplexed on this worker with it.
//
// A destructor is the shape this would be written in if it worked: it does not.
// The engine flags every live object as already destructed on its way into a
// fatal (zend_objects_store_mark_destructed, called just before the bailout), so
// nothing the release walk gives up can run a __destruct. A filter's onclose()
// is an ordinary method call made by the filter's own dtor, which that flag does
// not cover and which — unlike the filter() half — has no unclean-shutdown guard
// in front of it.
//
// No assertions and no test JSON: the suite line checks the 500 this produces.
// The request after it is the one that says whether the worker is still there.

// A worker never shuts a request down, so the error handler the TestCase of an
// earlier request installed is still standing on this thread — and it turns
// E_USER_ERROR into an exception, which unwinds cleanly and abandons nothing.
// Both fatals below have to be fatals.
set_error_handler(null);

if (!class_exists('OxPHPFatalOnCloseFilter', false)) {
    class OxPHPFatalOnCloseFilter extends php_user_filter
    {
        public function onClose(): void
        {
            // Recorded before the fatal so the request after this one can tell a
            // walk that reached the handle from one that never got to it.
            @file_put_contents('/tmp/oxphp-filter-close-marker', (string) time());

            trigger_error(
                'fatal raised from a stream filter the recovery walk was releasing',
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

// The filter map is per thread and this worker keeps it for its whole life, so
// registering twice would warn. Harmless either way, but the suite reads the
// body of the response this produces.
if (!in_array('oxphp-fatal-on-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-fatal-on-close', OxPHPFatalOnCloseFilter::class);
}

@unlink('/tmp/oxphp-filter-close-marker');

/**
 * The frame the fatal abandons. $handle is one of its compiled variables, so
 * the handle is given back by the recovery's release walk and by nothing else:
 * the function never returns, and a worker does not tear the request down.
 */
function oxphp_fatal_under_open_filter(): void
{
    $handle = fopen('php://memory', 'r+');
    // The chain takes a reference of its own, so discarding what this returns
    // leaves the filter alive until the stream is closed.
    stream_filter_append($handle, 'oxphp-fatal-on-close');

    trigger_error('the request fatal', E_USER_ERROR);
}

oxphp_fatal_under_open_filter();
