<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The other thing a filter's onclose() can do from inside the end of a request:
// drop the script's last reference to the very handle being closed.
//
// The end of a request closes the php://input handles still open on its body,
// and closing one calls onclose() on every userland filter in its chain. If that
// method releases the last reference to the handle, the resource the close is
// working through is freed while the close is still in progress, and whatever
// the close does next must not read it.
//
// The handle is held by a static property and nothing else, so the property is
// the last reference and onclose() is the one to drop it. Held there rather than
// by a local, because a local goes out of scope with the script, and that would
// close the handle before the end of the request ever looked for it.
//
// A pass here is weaker than it reads: a read of freed memory need not show at
// all in a release build, so this request answering and the next one being
// served on the same worker is all it can show. The next request checks the
// latter.

$t = new TestCase('php_input_filter_close_drops_handle', 'hooks');

if (!class_exists('OxPHPInputHandleHolder', false)) {
    final class OxPHPInputHandleHolder
    {
        /** @var resource|null */
        public static $handle = null;
    }

    class OxPHPInputDropOnCloseFilter extends php_user_filter
    {
        public function onClose(): void
        {
            @file_put_contents('/tmp/oxphp-input-filter-close-drop', (string) time());
            OxPHPInputHandleHolder::$handle = null;
        }

        /** @param resource $in @param resource $out */
        public function filter($in, $out, &$consumed, bool $closing): int
        {
            return PSFS_PASS_ON;
        }
    }
}

if (!in_array('oxphp-input-drop-on-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-input-drop-on-close', OxPHPInputDropOnCloseFilter::class);
}

// Guarded rather than @-silenced: the TestCase error handler ignores @ and
// turns the warning into an exception.
if (is_file('/tmp/oxphp-input-filter-close-drop')) {
    unlink('/tmp/oxphp-input-filter-close-drop');
}

OxPHPInputHandleHolder::$handle = fopen('php://input', 'r');
$t->assertTrue('php://input opened', is_resource(OxPHPInputHandleHolder::$handle));
stream_filter_append(OxPHPInputHandleHolder::$handle, 'oxphp-input-drop-on-close', STREAM_FILTER_READ);

$t->done();
